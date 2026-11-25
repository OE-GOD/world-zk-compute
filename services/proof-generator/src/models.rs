//! Model storage, hashing, and format auto-detection.
//!
//! Models are stored on disk under a configurable directory. Each model gets a
//! UUID-based directory containing the raw JSON and a metadata sidecar file.
//! The model hash is the SHA-256 digest of the JSON text as stored. For an
//! upload over HTTP that is the request's `model_json` re-serialised by
//! `serde_json`, not the bytes the client sent.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

/// Supported model format identifiers.
pub const FORMAT_XGBOOST: &str = "xgboost";
pub const FORMAT_LIGHTGBM: &str = "lightgbm";
pub const FORMAT_RANDOM_FOREST: &str = "random_forest";
pub const FORMAT_LOGISTIC_REGRESSION: &str = "logistic_regression";

/// Metadata stored alongside each model on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    pub id: String,
    pub name: String,
    pub format: String,
    pub model_hash: String,
    pub circuit_hash: String,
    pub active: bool,
    pub created_at: String,
    pub file_size_bytes: u64,
}

/// A loaded model held in memory.
#[derive(Debug, Clone)]
pub struct LoadedModel {
    pub id: String,
    pub name: String,
    pub format: String,
    pub model_hash: String,
    pub circuit_hash: String,
    pub active: bool,
    pub created_at: String,
    pub raw_json: String,
}

/// Thread-safe model store backed by disk and an in-memory index.
#[derive(Clone)]
pub struct ModelStore {
    /// Base directory for model storage.
    storage_dir: PathBuf,
    /// In-memory index of loaded models.
    models: Arc<RwLock<HashMap<String, LoadedModel>>>,
}

#[allow(dead_code)]
impl ModelStore {
    /// Create a new model store rooted at `storage_dir`.
    ///
    /// On creation, the directory is created if it does not exist, and any
    /// existing models are loaded from disk into the in-memory index.
    pub async fn new(storage_dir: PathBuf) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&storage_dir).await?;

        let store = Self {
            storage_dir,
            models: Arc::new(RwLock::new(HashMap::new())),
        };

        store.load_from_disk().await?;
        Ok(store)
    }

    /// Scan the storage directory and load all models into the in-memory index.
    async fn load_from_disk(&self) -> anyhow::Result<()> {
        let mut entries = tokio::fs::read_dir(&self.storage_dir).await?;
        let mut models = self.models.write().await;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            match load_model_dir(&path).await {
                Ok(model) => {
                    models.insert(model.id.clone(), model);
                }
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "Skipping model directory that could not be loaded"
                ),
            }
        }

        tracing::info!("Loaded {} models from disk", models.len());
        Ok(())
    }

    /// Store a new model. Returns the assigned metadata.
    ///
    /// The model JSON is written to disk, and the in-memory index is updated.
    /// Format is auto-detected if not provided. The `circuit_hash` is left as
    /// a placeholder until the real prover is integrated.
    #[allow(dead_code)]
    pub async fn add_model(
        &self,
        name: String,
        raw_json: String,
        format_hint: Option<String>,
    ) -> anyhow::Result<ModelMetadata> {
        let format = resolve_format(format_hint.as_deref(), &raw_json)?;

        let model_hash = compute_model_hash(&raw_json);

        // Check for duplicate hash
        {
            let models = self.models.read().await;
            for existing in models.values() {
                if existing.model_hash == model_hash {
                    anyhow::bail!(
                        "Model with identical hash already exists: id={}",
                        existing.id
                    );
                }
            }
        }

        let id = uuid::Uuid::new_v4().to_string();
        self.store_new(id, name, raw_json, format, true).await
    }

    /// Write a new model to disk under `id`, then add it to the index.
    async fn store_new(
        &self,
        id: String,
        name: String,
        raw_json: String,
        format: String,
        active: bool,
    ) -> anyhow::Result<ModelMetadata> {
        let model_hash = compute_model_hash(&raw_json);
        let now = chrono::Utc::now().to_rfc3339();

        // Placeholder circuit hash: in production, the prover would compute
        // the actual circuit hash from the built GKR circuit. For now we derive
        // a deterministic placeholder from the model hash so tests are stable.
        let circuit_hash = {
            let mut h = Sha256::new();
            h.update(b"circuit:");
            h.update(model_hash.as_bytes());
            format!("0x{}", hex::encode(h.finalize()))
        };

        let meta = ModelMetadata {
            id: id.clone(),
            name: name.clone(),
            format: format.clone(),
            model_hash: model_hash.clone(),
            circuit_hash: circuit_hash.clone(),
            active,
            created_at: now.clone(),
            file_size_bytes: raw_json.len() as u64,
        };

        // Persist to disk
        write_model_dir(&self.storage_dir.join(&id), &raw_json, &meta).await?;

        // Update in-memory index
        let loaded = LoadedModel {
            id: id.clone(),
            name,
            format,
            model_hash,
            circuit_hash,
            active,
            created_at: now,
            raw_json,
        };
        self.models.write().await.insert(id, loaded);

        Ok(meta)
    }

    /// List all models (active and inactive).
    pub async fn list_models(&self) -> Vec<ModelMetadata> {
        let models = self.models.read().await;
        models
            .values()
            .map(|m| ModelMetadata {
                id: m.id.clone(),
                name: m.name.clone(),
                format: m.format.clone(),
                model_hash: m.model_hash.clone(),
                circuit_hash: m.circuit_hash.clone(),
                active: m.active,
                created_at: m.created_at.clone(),
                file_size_bytes: m.raw_json.len() as u64,
            })
            .collect()
    }

    /// Get a model by ID.
    pub async fn get_model(&self, id: &str) -> Option<LoadedModel> {
        self.models.read().await.get(id).cloned()
    }

    /// Whether the model is active; `None` when the ID is unknown.
    pub async fn is_active(&self, id: &str) -> Option<bool> {
        self.models.read().await.get(id).map(|m| m.active)
    }

    /// Delete a model: its entry in the index, then its directory.
    ///
    /// This takes back a version whose upload did not complete.
    /// `deactivate_model` is how a model that was in use is retired.
    ///
    /// The entry goes whatever happens to the directory, so that the store
    /// never keeps a model its caller has given up. If the directory cannot
    /// be removed the error is returned, and what is left of it is loaded
    /// again (or skipped with a warning) at the next start.
    pub async fn remove_model(&self, id: &str) -> anyhow::Result<()> {
        let mut models = self.models.write().await;
        if models.remove(id).is_none() {
            anyhow::bail!("Model not found: {}", id);
        }
        match tokio::fs::remove_dir_all(self.storage_dir.join(id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Deactivate a model (soft delete).
    pub async fn deactivate_model(&self, id: &str) -> anyhow::Result<()> {
        let mut models = self.models.write().await;
        let model = models
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", id))?;

        // Update metadata on disk first; the in-memory flag follows only once
        // the write has succeeded.
        let meta = ModelMetadata {
            id: model.id.clone(),
            name: model.name.clone(),
            format: model.format.clone(),
            model_hash: model.model_hash.clone(),
            circuit_hash: model.circuit_hash.clone(),
            active: false,
            created_at: model.created_at.clone(),
            file_size_bytes: model.raw_json.len() as u64,
        };
        let meta_path = self.storage_dir.join(&model.id).join("metadata.json");
        write_atomic(&meta_path, serde_json::to_string_pretty(&meta)?.as_bytes()).await?;
        model.active = false;

        Ok(())
    }

    /// Activate a model (set active = true). The inverse of `deactivate_model`.
    pub async fn activate_model(&self, id: &str) -> anyhow::Result<()> {
        let mut models = self.models.write().await;
        let model = models
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", id))?;

        // Update metadata on disk first; the in-memory flag follows only once
        // the write has succeeded.
        let meta = ModelMetadata {
            id: model.id.clone(),
            name: model.name.clone(),
            format: model.format.clone(),
            model_hash: model.model_hash.clone(),
            circuit_hash: model.circuit_hash.clone(),
            active: true,
            created_at: model.created_at.clone(),
            file_size_bytes: model.raw_json.len() as u64,
        };
        let meta_path = self.storage_dir.join(&model.id).join("metadata.json");
        write_atomic(&meta_path, serde_json::to_string_pretty(&meta)?.as_bytes()).await?;
        model.active = true;

        Ok(())
    }

    /// Store a new model as inactive, without checking for global duplicate
    /// hashes.
    ///
    /// This is used by [`ModelRegistry`], which performs its own per-name
    /// duplicate checking and activates a version only once it is stored.
    /// The method is otherwise identical to `add_model`.
    pub async fn add_inactive_model_no_dup_check(
        &self,
        name: String,
        raw_json: String,
        format_hint: Option<String>,
    ) -> anyhow::Result<ModelMetadata> {
        let format = resolve_format(format_hint.as_deref(), &raw_json)?;
        let id = uuid::Uuid::new_v4().to_string();
        self.store_new(id, name, raw_json, format, false).await
    }
}

/// Create a new model's directory with its `model.json` and `metadata.json`.
/// If that fails on the way, what was created is removed again, so that a
/// model that could not be stored leaves no directory behind.
async fn write_model_dir(
    model_dir: &Path,
    raw_json: &str,
    meta: &ModelMetadata,
) -> anyhow::Result<()> {
    let written = async {
        tokio::fs::create_dir_all(model_dir).await?;
        write_atomic(&model_dir.join("model.json"), raw_json.as_bytes()).await?;
        write_atomic(
            &model_dir.join("metadata.json"),
            serde_json::to_string_pretty(meta)?.as_bytes(),
        )
        .await?;
        anyhow::Ok(())
    }
    .await;
    if written.is_err() {
        match tokio::fs::remove_dir_all(model_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                path = %model_dir.display(),
                error = %e,
                "Could not remove the directory of a model that was not stored"
            ),
        }
    }
    written
}

/// Read one model directory (`metadata.json` and `model.json`). The error
/// names the file that could not be read or parsed.
async fn load_model_dir(dir: &Path) -> anyhow::Result<LoadedModel> {
    let meta_path = dir.join("metadata.json");
    let meta_str = tokio::fs::read_to_string(&meta_path)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {}", meta_path.display(), e))?;
    let meta: ModelMetadata = serde_json::from_str(&meta_str)
        .map_err(|e| anyhow::anyhow!("{}: {}", meta_path.display(), e))?;

    let model_path = dir.join("model.json");
    let raw_json = tokio::fs::read_to_string(&model_path)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {}", model_path.display(), e))?;

    Ok(LoadedModel {
        id: meta.id,
        name: meta.name,
        format: meta.format,
        model_hash: meta.model_hash,
        circuit_hash: meta.circuit_hash,
        active: meta.active,
        created_at: meta.created_at,
        raw_json,
    })
}

/// Write `contents` to `path` through a temporary file next to it and a
/// rename, so the file at `path` is always either the previous version or the
/// complete new one, never a partial write.
///
/// A rename replaces a file whatever its permissions are. To keep this a
/// write to the existing file as far as permissions go, a file that cannot be
/// opened for writing is refused, and the new file is created with no more
/// than the mode of the one it replaces, and has exactly that mode when it
/// takes its place. A symbolic link at `path` is replaced by a regular file.
async fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let existing = match tokio::fs::OpenOptions::new().write(true).open(path).await {
        Ok(file) => Some(file.metadata().await?.permissions()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(std::io::Error::new(
                e.kind(),
                format!("{} cannot be written: {}", path.display(), e),
            ))
        }
    };

    let mut tmp_path = path.as_os_str().to_owned();
    tmp_path.push(".tmp");
    let tmp_path = PathBuf::from(tmp_path);
    // What an interrupted write left behind must not lend its mode to this one.
    match tokio::fs::remove_file(&tmp_path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let result = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(perms) = &existing {
            use std::os::unix::fs::PermissionsExt;
            options.mode(perms.mode() & 0o7777);
        }
        let mut file = options.open(&tmp_path).await?;
        file.write_all(contents).await?;
        file.sync_all().await?;
        drop(file);
        // The mode given at creation is cut by the umask; this makes it exact.
        if let Some(perms) = existing {
            tokio::fs::set_permissions(&tmp_path, perms).await?;
        }
        tokio::fs::rename(&tmp_path, path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp_path).await;
    }
    result
}

/// Compute the SHA-256 hash of a model's raw JSON, returned as a hex string
/// with `0x` prefix.
pub fn compute_model_hash(json: &str) -> String {
    let digest = Sha256::digest(json.as_bytes());
    format!("0x{}", hex::encode(digest))
}

/// Detect the model format by inspecting top-level keys in the JSON.
///
/// Returns one of the `FORMAT_*` constants on success.
pub fn detect_model_format(json_str: &str) -> anyhow::Result<&'static str> {
    let v: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;

    if v.get("learner").is_some() {
        return Ok(FORMAT_XGBOOST);
    }
    if v.get("tree_info").is_some() {
        return Ok(FORMAT_LIGHTGBM);
    }
    if v.get("n_estimators").is_some() || v.get("forest").is_some() {
        return Ok(FORMAT_RANDOM_FOREST);
    }
    if v.get("weights").is_some() {
        return Ok(FORMAT_LOGISTIC_REGRESSION);
    }
    // The prover's own serialized model (examples/xgboost-remainder/sample_model.json),
    // which its "xgboost" loader accepts next to the xgb.save_model format.
    if v.get("num_features").is_some() && v.get("trees").is_some() {
        return Ok(FORMAT_XGBOOST);
    }

    Err(anyhow::anyhow!(
        "Could not detect model format from JSON structure. \
         Supported: xgboost, lightgbm, random_forest, logistic_regression"
    ))
}

/// The format a model is stored under: the hint when one is given (and
/// known), otherwise the format detected from the JSON.
fn resolve_format(format_hint: Option<&str>, raw_json: &str) -> anyhow::Result<String> {
    match format_hint {
        Some(f) if f != "auto" => {
            validate_format(f)?;
            Ok(f.to_string())
        }
        _ => Ok(detect_model_format(raw_json)?.to_string()),
    }
}

/// Validate that a format string is one of the supported formats.
fn validate_format(format: &str) -> anyhow::Result<()> {
    match format {
        FORMAT_XGBOOST | FORMAT_LIGHTGBM | FORMAT_RANDOM_FOREST | FORMAT_LOGISTIC_REGRESSION => {
            Ok(())
        }
        other => Err(anyhow::anyhow!(
            "Unknown model format '{}'. Supported: xgboost, lightgbm, random_forest, logistic_regression",
            other
        )),
    }
}

/// Helpers for tests that need a metadata write to fail.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;

    /// Put a directory where a model's `metadata.json` is, so that the file
    /// cannot be written (unlike file permissions, this also stops root).
    /// Returns the file's contents for [`restore_metadata`].
    pub fn block_metadata_writes(storage_dir: &Path, id: &str) -> Vec<u8> {
        let meta_path = storage_dir.join(id).join("metadata.json");
        let saved = std::fs::read(&meta_path).unwrap();
        std::fs::remove_file(&meta_path).unwrap();
        std::fs::create_dir_all(meta_path.join("blocked")).unwrap();
        saved
    }

    /// Undo [`block_metadata_writes`].
    pub fn restore_metadata(storage_dir: &Path, id: &str, saved: &[u8]) {
        let meta_path = storage_dir.join(id).join("metadata.json");
        std::fs::remove_dir_all(&meta_path).unwrap();
        std::fs::write(&meta_path, saved).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{block_metadata_writes, restore_metadata};
    use super::*;
    use tempfile::TempDir;

    /// Collects everything logged while the returned guard is alive.
    #[derive(Clone, Default)]
    struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);

    impl LogCapture {
        fn start() -> (Self, tracing::subscriber::DefaultGuard) {
            let capture = Self::default();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(capture.clone())
                .with_ansi(false)
                .finish();
            let guard = tracing::subscriber::set_default(subscriber);
            (capture, guard)
        }

        fn lines(&self) -> Vec<String> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    impl std::io::Write for LogCapture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }

    #[test]
    fn test_detect_xgboost() {
        let json = r#"{"learner": {"gradient_booster": {"model": {"trees": []}}}}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_XGBOOST);
    }

    #[test]
    fn test_detect_lightgbm() {
        let json = r#"{"tree_info": [{"tree_index": 0}]}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_LIGHTGBM);
    }

    #[test]
    fn test_detect_random_forest() {
        let json = r#"{"n_estimators": 3, "trees": []}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_RANDOM_FOREST);
    }

    #[test]
    fn test_detect_logistic_regression() {
        let json = r#"{"weights": [0.5, -0.3], "bias": -0.2}"#;
        assert_eq!(
            detect_model_format(json).unwrap(),
            FORMAT_LOGISTIC_REGRESSION
        );
    }

    #[test]
    fn test_detect_prover_sample_model() {
        // The model file the prover's README and Dockerfile use.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/xgboost-remainder/sample_model.json");
        let json =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(detect_model_format(&json).unwrap(), FORMAT_XGBOOST);
    }

    #[test]
    fn test_detect_random_forest_with_trees_and_num_features() {
        // "n_estimators" is checked first, so a forest export is not taken for XGBoost.
        let json = r#"{"n_estimators": 3, "num_features": 4, "trees": []}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_RANDOM_FOREST);
    }

    #[test]
    fn test_detect_unknown_format() {
        let json = r#"{"unknown_key": 42}"#;
        assert!(detect_model_format(json).is_err());
    }

    #[test]
    fn test_detect_invalid_json() {
        assert!(detect_model_format("not json").is_err());
    }

    #[test]
    fn test_compute_model_hash_deterministic() {
        let json = r#"{"learner": {}}"#;
        let h1 = compute_model_hash(json);
        let h2 = compute_model_hash(json);
        assert_eq!(h1, h2);
        assert!(h1.starts_with("0x"));
        assert_eq!(h1.len(), 66); // 0x + 64 hex chars
    }

    #[test]
    fn test_compute_model_hash_different_inputs() {
        let h1 = compute_model_hash(r#"{"learner": {}}"#);
        let h2 = compute_model_hash(r#"{"learner": {"x": 1}}"#);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_validate_format_valid() {
        assert!(validate_format("xgboost").is_ok());
        assert!(validate_format("lightgbm").is_ok());
        assert!(validate_format("random_forest").is_ok());
        assert!(validate_format("logistic_regression").is_ok());
    }

    #[test]
    fn test_validate_format_invalid() {
        assert!(validate_format("pytorch").is_err());
        assert!(validate_format("").is_err());
    }

    #[tokio::test]
    async fn test_model_store_add_and_list() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();

        let json = r#"{"learner": {"gradient_booster": {"model": {"trees": []}}}}"#;
        let meta = store
            .add_model("test-model".into(), json.into(), None)
            .await
            .unwrap();

        assert_eq!(meta.name, "test-model");
        assert_eq!(meta.format, "xgboost");
        assert!(meta.active);
        assert!(meta.model_hash.starts_with("0x"));

        let models = store.list_models().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, meta.id);
    }

    #[tokio::test]
    async fn test_model_store_get() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();

        let json = r#"{"learner": {}}"#;
        let meta = store
            .add_model("get-test".into(), json.into(), None)
            .await
            .unwrap();

        let loaded = store.get_model(&meta.id).await.unwrap();
        assert_eq!(loaded.name, "get-test");
        assert_eq!(loaded.raw_json, json);
    }

    #[tokio::test]
    async fn test_model_store_deactivate() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();

        let json = r#"{"learner": {}}"#;
        let meta = store
            .add_model("deactivate-test".into(), json.into(), None)
            .await
            .unwrap();

        store.deactivate_model(&meta.id).await.unwrap();

        let loaded = store.get_model(&meta.id).await.unwrap();
        assert!(!loaded.active);
    }

    #[tokio::test]
    async fn test_deactivate_failed_write_leaves_model_active() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let meta = store
            .add_model("m".into(), r#"{"learner": {}}"#.into(), None)
            .await
            .unwrap();

        let saved = block_metadata_writes(tmp.path(), &meta.id);
        assert!(store.deactivate_model(&meta.id).await.is_err());
        restore_metadata(tmp.path(), &meta.id, &saved);

        // Memory still says what the disk says.
        assert!(store.get_model(&meta.id).await.unwrap().active);
        let reopened = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        assert!(reopened.get_model(&meta.id).await.unwrap().active);
    }

    #[tokio::test]
    async fn test_activate_failed_write_leaves_model_inactive() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let meta = store
            .add_model("m".into(), r#"{"learner": {}}"#.into(), None)
            .await
            .unwrap();
        store.deactivate_model(&meta.id).await.unwrap();

        let saved = block_metadata_writes(tmp.path(), &meta.id);
        assert!(store.activate_model(&meta.id).await.is_err());
        restore_metadata(tmp.path(), &meta.id, &saved);

        assert!(!store.get_model(&meta.id).await.unwrap().active);
        let reopened = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        assert!(!reopened.get_model(&meta.id).await.unwrap().active);
    }

    /// A metadata update must never touch the existing file in place: a hard
    /// link to it keeps the old contents, and no temporary file is left over.
    #[tokio::test]
    async fn test_metadata_update_replaces_the_file() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let meta = store
            .add_model("m".into(), r#"{"learner": {}}"#.into(), None)
            .await
            .unwrap();

        let model_dir = tmp.path().join(&meta.id);
        let meta_path = model_dir.join("metadata.json");
        let before = std::fs::read_to_string(&meta_path).unwrap();
        let link = tmp.path().join("metadata-before.json");
        std::fs::hard_link(&meta_path, &link).unwrap();

        store.deactivate_model(&meta.id).await.unwrap();

        assert_eq!(std::fs::read_to_string(&link).unwrap(), before);
        let after: ModelMetadata =
            serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
        assert!(!after.active);

        let mut files: Vec<String> = std::fs::read_dir(&model_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(files, ["metadata.json", "model.json"]);
    }

    /// The replacement keeps the permissions of the file it replaces, and a
    /// file that cannot be written in place is refused.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_atomic_write_keeps_permissions_and_refuses_unwritable_file() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("metadata.json");
        std::fs::write(&path, b"old").unwrap();
        let set_mode =
            |mode| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        let mode_of = |file: &Path| std::fs::metadata(file).unwrap().permissions().mode() & 0o777;

        set_mode(0o600);
        write_atomic(&path, b"new").await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(mode_of(&path), 0o600);

        // A mode the umask would cut is kept as well.
        set_mode(0o666);
        write_atomic(&path, b"new").await.unwrap();
        assert_eq!(mode_of(&path), 0o666);

        // Read-only, and writable by the group only: neither can be written
        // in place by its owner. (Root can write both in place, and is then
        // allowed to replace them too.)
        for mode in [0o444, 0o464] {
            set_mode(mode);
            let writable_in_place = std::fs::OpenOptions::new().write(true).open(&path).is_ok();
            let result = write_atomic(&path, b"newer").await;
            assert_eq!(result.is_ok(), writable_in_place, "mode {mode:o}");
            if let Err(err) = result {
                assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
                assert_eq!(std::fs::read(&path).unwrap(), b"new");
            }
            assert!(!tmp.path().join("metadata.json.tmp").exists());
        }

        // A temporary file left by an interrupted write, with another mode,
        // does not pass that mode on.
        set_mode(0o600);
        let leftover = tmp.path().join("metadata.json.tmp");
        std::fs::write(&leftover, b"partial").unwrap();
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o666)).unwrap();
        write_atomic(&path, b"last").await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"last");
        assert_eq!(mode_of(&path), 0o600);
        assert!(!leftover.exists());
    }

    /// A model whose files cannot all be written leaves no directory behind
    /// and is not in the index.
    #[tokio::test]
    async fn test_model_that_cannot_be_stored_leaves_no_directory() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let store_new = |id: &str| {
            store.store_new(
                id.to_string(),
                "m".to_string(),
                r#"{"learner": {}}"#.to_string(),
                FORMAT_XGBOOST.to_string(),
                false,
            )
        };

        // metadata.json cannot be written: a directory is in its place.
        let model_dir = tmp.path().join("new-model");
        std::fs::create_dir_all(model_dir.join("metadata.json").join("blocked")).unwrap();
        assert!(store_new("new-model").await.is_err());
        assert!(
            !model_dir.exists(),
            "a half-written model directory is left"
        );
        assert!(store.list_models().await.is_empty());

        // A model that can be written is kept.
        store_new("ok").await.unwrap();
        assert!(tmp.path().join("ok").join("metadata.json").is_file());
        assert_eq!(store.is_active("ok").await, Some(false));
    }

    /// A version can be stored inactive, and a stored version can be taken
    /// back: its directory and its entry both go.
    #[tokio::test]
    async fn test_store_inactive_and_remove() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let meta = store
            .add_inactive_model_no_dup_check("m".into(), r#"{"learner": {}}"#.into(), None)
            .await
            .unwrap();
        assert!(!meta.active);
        assert_eq!(store.is_active(&meta.id).await, Some(false));
        let reopened = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        assert_eq!(reopened.is_active(&meta.id).await, Some(false));

        store.remove_model(&meta.id).await.unwrap();
        assert_eq!(store.is_active(&meta.id).await, None);
        assert!(!tmp.path().join(&meta.id).exists());
        assert!(store.remove_model(&meta.id).await.is_err());
        let reopened = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        assert!(reopened.list_models().await.is_empty());

        // A model whose directory is already gone is removed from the index
        // all the same.
        let meta = store
            .add_inactive_model_no_dup_check("m".into(), r#"{"learner": {}}"#.into(), None)
            .await
            .unwrap();
        std::fs::remove_dir_all(tmp.path().join(&meta.id)).unwrap();
        store.remove_model(&meta.id).await.unwrap();
        assert_eq!(store.is_active(&meta.id).await, None);
    }

    /// The index entry goes even when the directory cannot be removed, so
    /// that the store does not keep a model its caller has given up.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_remove_drops_the_entry_when_the_directory_stays() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let meta = store
            .add_inactive_model_no_dup_check("m".into(), r#"{"learner": {}}"#.into(), None)
            .await
            .unwrap();

        // The model directory cannot be emptied.
        let dir = tmp.path().join(&meta.id);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = store.remove_model(&meta.id).await;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        if result.is_ok() {
            // Running as root, which the directory mode does not stop.
            return;
        }

        assert_eq!(store.is_active(&meta.id).await, None);
        assert!(store.list_models().await.is_empty());
    }

    #[tokio::test]
    async fn test_load_warns_about_each_skipped_directory() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let kept = store
            .add_model("kept".into(), r#"{"learner": {}}"#.into(), None)
            .await
            .unwrap();
        let cut = store
            .add_model("cut".into(), r#"{"learner": {"x": 1}}"#.into(), None)
            .await
            .unwrap();

        // What an interrupted in-place write would have left behind.
        let cut_meta = tmp.path().join(&cut.id).join("metadata.json");
        let full = std::fs::read(&cut_meta).unwrap();
        std::fs::write(&cut_meta, &full[..full.len() / 2]).unwrap();
        // A directory with no model files at all.
        let empty_dir = tmp.path().join("no-model-here");
        std::fs::create_dir(&empty_dir).unwrap();

        let (logs, _guard) = LogCapture::start();
        let reopened = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let models = reopened.list_models().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, kept.id);

        let warnings: Vec<String> = logs
            .lines()
            .into_iter()
            .filter(|line| line.contains("WARN"))
            .collect();
        assert_eq!(warnings.len(), 2, "{warnings:#?}");
        let about = |dir: &std::path::Path| {
            let dir = dir.join("metadata.json").display().to_string();
            warnings
                .iter()
                .find(|line| line.contains(&dir))
                .unwrap_or_else(|| panic!("no warning names {dir}: {warnings:#?}"))
        };
        assert!(about(&tmp.path().join(&cut.id)).contains("EOF while parsing"));
        assert!(about(&empty_dir).contains("os error 2"));
    }

    #[tokio::test]
    async fn test_model_store_duplicate_hash_rejected() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();

        let json = r#"{"learner": {}}"#;
        store
            .add_model("first".into(), json.into(), None)
            .await
            .unwrap();

        let result = store.add_model("second".into(), json.into(), None).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("identical hash already exists"));
    }

    #[tokio::test]
    async fn test_model_store_persistence() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();

        // Create store and add model
        let store = ModelStore::new(path.clone()).await.unwrap();
        let json = r#"{"learner": {}}"#;
        let meta = store
            .add_model("persist-test".into(), json.into(), None)
            .await
            .unwrap();

        // Create a new store from the same directory
        let store2 = ModelStore::new(path).await.unwrap();
        let models = store2.list_models().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, meta.id);
        assert_eq!(models[0].name, "persist-test");
    }

    #[tokio::test]
    async fn test_model_store_explicit_format() {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();

        // An explicit format is stored as given, without detection.
        let json = r#"{"weights": [1.0]}"#;
        let meta = store
            .add_model(
                "forced-format".into(),
                json.into(),
                Some("logistic_regression".into()),
            )
            .await
            .unwrap();
        assert_eq!(meta.format, "logistic_regression");
    }
}
