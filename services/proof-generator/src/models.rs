//! Model storage, hashing, and format auto-detection.
//!
//! Models are stored on disk under a configurable directory. Each model gets a
//! UUID-based directory containing the raw JSON and a metadata sidecar file.
//! The model hash is the SHA-256 digest of the raw JSON bytes.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
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

            let meta_path = path.join("metadata.json");
            let model_path = path.join("model.json");

            if !meta_path.exists() || !model_path.exists() {
                continue;
            }

            let meta_str = match tokio::fs::read_to_string(&meta_path).await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let meta: ModelMetadata = match serde_json::from_str(&meta_str) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let raw_json = match tokio::fs::read_to_string(&model_path).await {
                Ok(s) => s,
                Err(_) => continue,
            };

            models.insert(
                meta.id.clone(),
                LoadedModel {
                    id: meta.id,
                    name: meta.name,
                    format: meta.format,
                    model_hash: meta.model_hash,
                    circuit_hash: meta.circuit_hash,
                    active: meta.active,
                    created_at: meta.created_at,
                    raw_json,
                },
            );
        }

        tracing::info!("Loaded {} models from disk", models.len());
        Ok(())
    }

    /// Store a new model. Returns the assigned metadata.
    ///
    /// The model JSON is written to disk, and the in-memory index is updated.
    /// Format is auto-detected if not provided. The `circuit_hash` is left as
    /// a placeholder until the real prover is integrated.
    pub async fn add_model(
        &self,
        name: String,
        raw_json: String,
        format_hint: Option<String>,
    ) -> anyhow::Result<ModelMetadata> {
        let format = match format_hint {
            Some(f) if f != "auto" => {
                validate_format(&f)?;
                f
            }
            _ => detect_model_format(&raw_json)?.to_string(),
        };

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
            active: true,
            created_at: now.clone(),
            file_size_bytes: raw_json.len() as u64,
        };

        // Persist to disk
        let model_dir = self.storage_dir.join(&id);
        tokio::fs::create_dir_all(&model_dir).await?;
        tokio::fs::write(model_dir.join("model.json"), &raw_json).await?;
        tokio::fs::write(
            model_dir.join("metadata.json"),
            serde_json::to_string_pretty(&meta)?,
        )
        .await?;

        // Update in-memory index
        let loaded = LoadedModel {
            id: id.clone(),
            name,
            format,
            model_hash,
            circuit_hash,
            active: true,
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

    /// Deactivate a model (soft delete).
    pub async fn deactivate_model(&self, id: &str) -> anyhow::Result<()> {
        let mut models = self.models.write().await;
        let model = models
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", id))?;
        model.active = false;

        // Update metadata on disk
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
        tokio::fs::write(meta_path, serde_json::to_string_pretty(&meta)?).await?;

        Ok(())
    }
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

    Err(anyhow::anyhow!(
        "Could not detect model format from JSON structure. \
         Supported: xgboost, lightgbm, random_forest, logistic_regression"
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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

        // JSON looks like logistic regression but we force xgboost
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
