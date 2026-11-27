//! Model registry with versioning, activation controls, and rollback.
//!
//! Built on top of [`ModelStore`] to add:
//!
//! - **Versioning**: Each upload of the same model name creates a new version
//! - **Activation**: Only active model versions can be used for proving
//! - **Rollback**: Quickly revert to a previous version
//!
//! The registry maintains a secondary index from model name to an ordered list
//! of version IDs (oldest-first). Activation state is tracked per version ID.
//! The activated flag is persisted via the underlying `ModelStore`; the time
//! of the activation (`activated_at`) is kept in memory only. After a restart
//! `activated_at` is the creation time for an active version and `None` for
//! an inactive one.
//!
//! Every operation that changes the registry runs in a task of its own (see
//! `run_to_completion`) and switches versions through `make_only_active`,
//! which never leaves a name without an active version on the way.

use crate::models::{compute_model_hash, LoadedModel, ModelMetadata, ModelStore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Per-version metadata returned by the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    /// The model ID (UUID) for this version.
    pub id: String,
    /// Human-readable model name (shared across versions).
    pub name: String,
    /// Sequential version number (1-based, increases with each upload).
    pub version: u32,
    /// ISO-8601 timestamp when this version was created.
    pub created_at: String,
    /// ISO-8601 timestamp when this version was last activated, or `None`.
    ///
    /// Kept in memory only: after a restart it is the creation time for an
    /// active version and `None` for an inactive one.
    pub activated_at: Option<String>,
    /// Whether this version is currently active for proving.
    pub is_active: bool,
    /// SHA-256 hash of the raw model JSON.
    pub model_hash: String,
    /// Deterministic circuit hash derived from the model hash.
    pub circuit_hash: String,
    /// Detected or specified model format.
    pub format: String,
    /// Size of the raw model JSON in bytes.
    pub file_size_bytes: u64,
}

/// Internal bookkeeping per version.
#[derive(Debug, Clone)]
struct VersionEntry {
    /// Model ID (UUID) in the underlying store.
    id: String,
    /// Sequential version number.
    version: u32,
    /// When activated (if ever).
    activated_at: Option<String>,
}

/// Internal bookkeeping per model name.
#[derive(Debug, Clone)]
struct NameEntry {
    /// Ordered list of version entries (oldest first).
    versions: Vec<VersionEntry>,
}

/// Run a change to the registry in a task of its own and wait for it.
///
/// A change is several writes, to disk and to memory. The HTTP handler that
/// asks for it is dropped when its client disconnects, and a change dropped
/// between two writes would leave them disagreeing. The task runs to its end
/// whether or not the caller is still waiting. (The end of the process still
/// cuts it off; a switch cut off that way leaves the new version active on
/// disk next to those that were, never none.)
async fn run_to_completion<T, F>(change: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
{
    match tokio::spawn(change).await {
        Ok(result) => result,
        // The task panicked; nothing cancels it. An `io::Error`, so that the
        // HTTP handlers answer 500.
        Err(e) => Err(anyhow::Error::new(std::io::Error::from(e))),
    }
}

/// Model registry wrapping a [`ModelStore`] with versioning support.
#[derive(Clone)]
pub struct ModelRegistry {
    store: ModelStore,
    /// Maps model name -> ordered versions.
    names: Arc<RwLock<HashMap<String, NameEntry>>>,
}

#[allow(dead_code)]
impl ModelRegistry {
    /// Create a new registry wrapping the given store.
    ///
    /// Scans existing models to reconstruct the name->versions index.
    pub async fn new(store: ModelStore) -> Self {
        let registry = Self {
            store,
            names: Arc::new(RwLock::new(HashMap::new())),
        };
        registry.rebuild_index().await;
        registry
    }

    /// Rebuild the name->versions index from the underlying store.
    async fn rebuild_index(&self) {
        let all_models = self.store.list_models().await;
        let mut names: HashMap<String, NameEntry> = HashMap::new();

        // Sort by created_at so version numbers are assigned in order.
        let mut sorted: Vec<ModelMetadata> = all_models;
        sorted.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        for meta in sorted {
            let entry = names.entry(meta.name.clone()).or_insert_with(|| NameEntry {
                versions: Vec::new(),
            });
            let version = entry.versions.len() as u32 + 1;
            entry.versions.push(VersionEntry {
                id: meta.id.clone(),
                version,
                activated_at: if meta.active {
                    Some(meta.created_at.clone())
                } else {
                    None
                },
            });
        }

        *self.names.write().await = names;
    }

    /// Access the underlying store (e.g., for `get_model` to retrieve raw JSON).
    #[allow(dead_code)]
    pub fn store(&self) -> &ModelStore {
        &self.store
    }

    /// Upload a new model version. If a model with the same name already
    /// exists, this creates the next version. The new version is activated
    /// by default and any previously active version of the same name is
    /// deactivated. An upload that fails leaves the registry as it was.
    ///
    /// Unlike the raw `ModelStore::add_model`, duplicate hashes are allowed
    /// across different names (but still rejected within the same name).
    pub async fn upload(
        &self,
        name: String,
        raw_json: String,
        format_hint: Option<String>,
    ) -> anyhow::Result<VersionInfo> {
        let registry = self.clone();
        run_to_completion(async move { registry.upload_now(name, raw_json, format_hint).await })
            .await
    }

    async fn upload_now(
        &self,
        name: String,
        raw_json: String,
        format_hint: Option<String>,
    ) -> anyhow::Result<VersionInfo> {
        let model_hash = compute_model_hash(&raw_json);

        // The index stays write-locked from the duplicate check to the insert,
        // so two concurrent uploads of the same model cannot both pass the check.
        let mut names = self.names.write().await;

        // Check for duplicate hash within the same model name.
        if let Some(entry) = names.get(&name) {
            for ve in &entry.versions {
                if let Some(m) = self.store.get_model(&ve.id).await {
                    if m.model_hash == model_hash {
                        anyhow::bail!(
                            "Version with identical model hash already exists for '{}': id={}",
                            name,
                            ve.id
                        );
                    }
                }
            }
        }

        // Store the new version inactive (bypassing the store's global
        // duplicate check: ours is above). Until it is complete on disk no
        // other version is touched, so a model the store refuses or cannot
        // write changes nothing in the registry (and the store removes what
        // it had written of it).
        let meta = self
            .store
            .add_inactive_model_no_dup_check(name.clone(), raw_json, format_hint)
            .await?;

        // Switch over to it. Each version keeps its activated_at timestamp
        // for history.
        let earlier: Vec<String> = match names.get(&name) {
            Some(entry) => entry.versions.iter().map(|ve| ve.id.clone()).collect(),
            None => Vec::new(),
        };
        if let Err(e) = self.make_only_active(&meta.id, &earlier).await {
            // Not switched: take the stored version back, so that the failed
            // upload leaves nothing behind.
            if let Err(remove) = self.store.remove_model(&meta.id).await {
                tracing::warn!(
                    model_id = %meta.id,
                    error = %remove,
                    "could not remove the version stored by a failed upload"
                );
            }
            return Err(e);
        }

        let now = chrono::Utc::now().to_rfc3339();

        let entry = names.entry(name.clone()).or_insert_with(|| NameEntry {
            versions: Vec::new(),
        });
        let version = entry.versions.len() as u32 + 1;
        entry.versions.push(VersionEntry {
            id: meta.id.clone(),
            version,
            activated_at: Some(now.clone()),
        });
        drop(names);

        Ok(VersionInfo {
            id: meta.id,
            name,
            version,
            created_at: meta.created_at,
            activated_at: Some(now),
            is_active: true,
            model_hash: meta.model_hash,
            circuit_hash: meta.circuit_hash,
            format: meta.format,
            file_size_bytes: meta.file_size_bytes,
        })
    }

    /// Make `id` the one active version among itself and `others` (the IDs
    /// of the other versions of its name; `id` itself is skipped if listed).
    ///
    /// `id` is activated before the others are deactivated, so the name is
    /// never without an active version on the way. If a write fails, the
    /// versions changed so far are put back and the error is returned.
    async fn make_only_active(&self, id: &str, others: &[String]) -> anyhow::Result<()> {
        let was_active = self.store.is_active(id).await == Some(true);
        self.store.activate_model(id).await?;

        let mut deactivated: Vec<&str> = Vec::new();
        for other in others.iter().filter(|other| other.as_str() != id) {
            if self.store.is_active(other).await != Some(true) {
                continue;
            }
            if let Err(e) = self.store.deactivate_model(other).await {
                for done in deactivated {
                    if let Err(undo) = self.store.activate_model(done).await {
                        tracing::warn!(
                            model_id = %done,
                            error = %undo,
                            "could not re-activate a version after a failed switch"
                        );
                    }
                }
                if !was_active {
                    if let Err(undo) = self.store.deactivate_model(id).await {
                        tracing::warn!(
                            model_id = %id,
                            error = %undo,
                            "could not deactivate a version again after a failed switch"
                        );
                    }
                }
                return Err(e);
            }
            deactivated.push(other);
        }
        Ok(())
    }

    /// List all versions for a given model name, ordered oldest-first.
    pub async fn list_versions(&self, name: &str) -> Option<Vec<VersionInfo>> {
        let names = self.names.read().await;
        let entry = names.get(name)?;

        let mut result = Vec::with_capacity(entry.versions.len());
        for ve in &entry.versions {
            if let Some(m) = self.store.get_model(&ve.id).await {
                result.push(VersionInfo {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: ve.version,
                    created_at: m.created_at.clone(),
                    activated_at: ve.activated_at.clone(),
                    is_active: m.active,
                    model_hash: m.model_hash.clone(),
                    circuit_hash: m.circuit_hash.clone(),
                    format: m.format.clone(),
                    file_size_bytes: m.raw_json.len() as u64,
                });
            }
        }

        Some(result)
    }

    /// Activate a specific model version by its ID.
    ///
    /// Deactivates any other active version of the same model name. An
    /// activation that fails leaves the registry as it was.
    pub async fn activate(&self, id: &str) -> anyhow::Result<VersionInfo> {
        let registry = self.clone();
        let id = id.to_string();
        run_to_completion(async move { registry.activate_now(&id).await }).await
    }

    async fn activate_now(&self, id: &str) -> anyhow::Result<VersionInfo> {
        let mut names = self.names.write().await;
        self.activate_locked(&mut names, id).await
    }

    /// `activate` for a caller that holds the index lock.
    async fn activate_locked(
        &self,
        names: &mut HashMap<String, NameEntry>,
        id: &str,
    ) -> anyhow::Result<VersionInfo> {
        let model = self
            .store
            .get_model(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", id))?;
        let entry = names
            .get_mut(&model.name)
            .ok_or_else(|| anyhow::anyhow!("Model name not found in registry: {}", model.name))?;
        // The store can hold a model the index does not list (one added to
        // the store directly). It is not a version of the name, and
        // activating it must not retire the versions that are.
        let version = entry
            .versions
            .iter()
            .find(|ve| ve.id == id)
            .map(|ve| ve.version)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", id))?;

        // Deactivate all other versions of this name, activate this one.
        let versions: Vec<String> = entry.versions.iter().map(|ve| ve.id.clone()).collect();
        self.make_only_active(id, &versions).await?;
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(ve) = entry.versions.iter_mut().find(|ve| ve.id == id) {
            ve.activated_at = Some(now.clone());
        }

        Ok(VersionInfo {
            id: model.id,
            name: model.name,
            version,
            created_at: model.created_at,
            activated_at: Some(now),
            is_active: true,
            model_hash: model.model_hash,
            circuit_hash: model.circuit_hash,
            format: model.format,
            file_size_bytes: model.raw_json.len() as u64,
        })
    }

    /// Deactivate a specific model version by its ID.
    pub async fn deactivate(&self, id: &str) -> anyhow::Result<VersionInfo> {
        let registry = self.clone();
        let id = id.to_string();
        run_to_completion(async move { registry.deactivate_now(&id).await }).await
    }

    async fn deactivate_now(&self, id: &str) -> anyhow::Result<VersionInfo> {
        // The index lock keeps this apart from a switch between versions
        // that is under way.
        let names = self.names.write().await;
        let model = self
            .store
            .get_model(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", id))?;
        let entry = names
            .get(&model.name)
            .ok_or_else(|| anyhow::anyhow!("Model name not found in registry"))?;
        let ve = entry
            .versions
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| anyhow::anyhow!("Version entry not found"))?;

        self.store.deactivate_model(id).await?;

        Ok(VersionInfo {
            id: model.id.clone(),
            name: model.name.clone(),
            version: ve.version,
            created_at: model.created_at.clone(),
            activated_at: ve.activated_at.clone(),
            is_active: false,
            model_hash: model.model_hash.clone(),
            circuit_hash: model.circuit_hash.clone(),
            format: model.format.clone(),
            file_size_bytes: model.raw_json.len() as u64,
        })
    }

    /// Rollback to the previous version of a model name.
    ///
    /// If the currently active version is v_N, this deactivates v_N and
    /// activates v_{N-1}. If no version is active, the latest version is
    /// treated as "current" and the second-to-last is activated.
    ///
    /// Returns the newly activated version info.
    pub async fn rollback(&self, name: &str) -> anyhow::Result<VersionInfo> {
        let registry = self.clone();
        let name = name.to_string();
        run_to_completion(async move { registry.rollback_now(&name).await }).await
    }

    async fn rollback_now(&self, name: &str) -> anyhow::Result<VersionInfo> {
        // One lock from choosing the target to the switch, so that an upload
        // cannot land in between and be retired by the rollback.
        let mut names = self.names.write().await;
        let entry = names
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Model name not found: {}", name))?;

        if entry.versions.len() < 2 {
            anyhow::bail!(
                "Cannot rollback '{}': need at least 2 versions, found {}",
                name,
                entry.versions.len()
            );
        }

        // Find the currently active version index.
        let mut active_idx = None;
        for (i, ve) in entry.versions.iter().enumerate() {
            if self.store.is_active(&ve.id).await == Some(true) {
                active_idx = Some(i);
            }
        }

        // If no active version, treat the last as "current".
        let current_idx = active_idx.unwrap_or(entry.versions.len() - 1);
        if current_idx == 0 {
            anyhow::bail!(
                "Cannot rollback '{}': already at the earliest version (v1)",
                name
            );
        }

        let prev_id = entry.versions[current_idx - 1].id.clone();

        // Activate the previous version (this also deactivates the current).
        self.activate_locked(&mut names, &prev_id).await
    }

    /// Get the currently active model for a given name.
    #[allow(dead_code)]
    pub async fn get_active_model(&self, name: &str) -> Option<LoadedModel> {
        let names = self.names.read().await;
        let entry = names.get(name)?;

        for ve in entry.versions.iter().rev() {
            if let Some(m) = self.store.get_model(&ve.id).await {
                if m.active {
                    return Some(m);
                }
            }
        }
        None
    }

    /// Get version info for a specific model ID.
    #[allow(dead_code)]
    pub async fn get_version_info(&self, id: &str) -> Option<VersionInfo> {
        let model = self.store.get_model(id).await?;
        let names = self.names.read().await;
        let entry = names.get(&model.name)?;
        let ve = entry.versions.iter().find(|v| v.id == id)?;

        Some(VersionInfo {
            id: model.id.clone(),
            name: model.name.clone(),
            version: ve.version,
            created_at: model.created_at.clone(),
            activated_at: ve.activated_at.clone(),
            is_active: model.active,
            model_hash: model.model_hash.clone(),
            circuit_hash: model.circuit_hash.clone(),
            format: model.format.clone(),
            file_size_bytes: model.raw_json.len() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::test_support::{block_metadata_writes, restore_metadata};
    use tempfile::TempDir;

    async fn make_registry() -> (ModelRegistry, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let registry = ModelRegistry::new(store).await;
        (registry, tmp)
    }

    fn xgboost_json(unique: &str) -> String {
        format!(
            r#"{{"learner": {{"gradient_booster": {{"model": {{"trees": [], "id": "{}"}}}}}}}}"#,
            unique
        )
    }

    // ----- Upload creates versions -----

    #[tokio::test]
    async fn test_upload_creates_v1() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("my-model".into(), xgboost_json("a"), None)
            .await
            .unwrap();

        assert_eq!(v1.version, 1);
        assert_eq!(v1.name, "my-model");
        assert!(v1.is_active);
        assert!(v1.activated_at.is_some());
    }

    #[tokio::test]
    async fn test_upload_same_name_creates_v2() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("my-model".into(), xgboost_json("a"), None)
            .await
            .unwrap();
        let v2 = reg
            .upload("my-model".into(), xgboost_json("b"), None)
            .await
            .unwrap();

        assert_eq!(v1.version, 1);
        assert_eq!(v2.version, 2);
        assert_ne!(v1.id, v2.id);
        assert_ne!(v1.model_hash, v2.model_hash);

        // v2 is active, v1 is now deactivated.
        let v1_model = reg.store().get_model(&v1.id).await.unwrap();
        let v2_model = reg.store().get_model(&v2.id).await.unwrap();
        assert!(!v1_model.active);
        assert!(v2_model.active);
    }

    #[tokio::test]
    async fn test_upload_duplicate_hash_same_name_rejected() {
        let (reg, _tmp) = make_registry().await;
        let json = xgboost_json("dup");
        reg.upload("my-model".into(), json.clone(), None)
            .await
            .unwrap();

        let result = reg.upload("my-model".into(), json, None).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("identical model hash"));
    }

    #[tokio::test]
    async fn test_concurrent_identical_uploads_accept_one() {
        let (reg, _tmp) = make_registry().await;
        let json = xgboost_json("same");
        let (a, b) = tokio::join!(
            reg.upload("my-model".into(), json.clone(), None),
            reg.upload("my-model".into(), json, None)
        );

        assert!(
            a.is_ok() != b.is_ok(),
            "exactly one upload must be accepted: {a:?} / {b:?}"
        );
        let rejected = a.err().or(b.err()).unwrap();
        assert!(rejected.to_string().contains("identical model hash"));
        assert_eq!(reg.list_versions("my-model").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_upload_same_hash_different_name_ok() {
        let (reg, _tmp) = make_registry().await;
        let json = xgboost_json("shared");
        reg.upload("model-a".into(), json.clone(), None)
            .await
            .unwrap();

        // Same JSON under a different name is allowed.
        let v = reg.upload("model-b".into(), json, None).await.unwrap();
        assert_eq!(v.version, 1);
        assert_eq!(v.name, "model-b");
    }

    // ----- List versions -----

    #[tokio::test]
    async fn test_list_versions() {
        let (reg, _tmp) = make_registry().await;
        reg.upload("mv".into(), xgboost_json("1"), None)
            .await
            .unwrap();
        reg.upload("mv".into(), xgboost_json("2"), None)
            .await
            .unwrap();
        reg.upload("mv".into(), xgboost_json("3"), None)
            .await
            .unwrap();

        let versions = reg.list_versions("mv").await.unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[1].version, 2);
        assert_eq!(versions[2].version, 3);

        // Only v3 should be active.
        assert!(!versions[0].is_active);
        assert!(!versions[1].is_active);
        assert!(versions[2].is_active);
    }

    #[tokio::test]
    async fn test_list_versions_unknown_name() {
        let (reg, _tmp) = make_registry().await;
        assert!(reg.list_versions("nonexistent").await.is_none());
    }

    // ----- Activate / Deactivate -----

    #[tokio::test]
    async fn test_activate_specific_version() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("act".into(), xgboost_json("x"), None)
            .await
            .unwrap();
        let v2 = reg
            .upload("act".into(), xgboost_json("y"), None)
            .await
            .unwrap();

        // v2 is active. Activate v1.
        let info = reg.activate(&v1.id).await.unwrap();
        assert!(info.is_active);
        assert_eq!(info.version, 1);

        // v2 should now be deactivated.
        let v2_model = reg.store().get_model(&v2.id).await.unwrap();
        assert!(!v2_model.active);
    }

    #[tokio::test]
    async fn test_deactivate() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("deact".into(), xgboost_json("z"), None)
            .await
            .unwrap();

        let info = reg.deactivate(&v1.id).await.unwrap();
        assert!(!info.is_active);

        let model = reg.store().get_model(&v1.id).await.unwrap();
        assert!(!model.active);
    }

    /// `activated_at` is not kept on disk: a reopened registry reports the
    /// creation time for the active version and `None` for the others.
    #[tokio::test]
    async fn test_reopened_registry_reports_activated_at_from_the_active_flag() {
        let (reg, tmp) = make_registry().await;
        let v1 = reg
            .upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();
        reg.upload("m".into(), xgboost_json("2"), None)
            .await
            .unwrap();
        reg.upload("m".into(), xgboost_json("3"), None)
            .await
            .unwrap();
        // v1 is active again, and the other two were active once.
        reg.activate(&v1.id).await.unwrap();

        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let reopened = ModelRegistry::new(store).await;
        let versions = reopened.list_versions("m").await.unwrap();

        let activated: Vec<(u32, bool, Option<&str>)> = versions
            .iter()
            .map(|v| (v.version, v.is_active, v.activated_at.as_deref()))
            .collect();
        assert_eq!(
            activated,
            [
                (1, true, Some(versions[0].created_at.as_str())),
                (2, false, None),
                (3, false, None),
            ]
        );
    }

    /// `(version, is_active)` for every version of `name`.
    async fn active_flags(reg: &ModelRegistry, name: &str) -> Vec<(u32, bool)> {
        let versions = reg.list_versions(name).await.unwrap();
        versions.iter().map(|v| (v.version, v.is_active)).collect()
    }

    #[tokio::test]
    async fn test_upload_reports_failed_deactivation_of_previous_version() {
        let (reg, tmp) = make_registry().await;
        let v1 = reg
            .upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();

        let saved = block_metadata_writes(tmp.path(), &v1.id);
        let result = reg.upload("m".into(), xgboost_json("2"), None).await;
        restore_metadata(tmp.path(), &v1.id, &saved);

        assert!(result.is_err(), "v1 could not be deactivated: {result:?}");
        // The upload changed nothing: v1 is still the one active version,
        // and what the registry reports is what a restart finds on disk.
        let in_memory = active_flags(&reg, "m").await;
        assert_eq!(in_memory, [(1, true)]);
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let reopened = ModelRegistry::new(store).await;
        assert_eq!(active_flags(&reopened, "m").await, in_memory);
    }

    #[tokio::test]
    async fn test_activate_reports_failed_deactivation_of_other_version() {
        let (reg, tmp) = make_registry().await;
        let v1 = reg
            .upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();
        let v2 = reg
            .upload("m".into(), xgboost_json("2"), None)
            .await
            .unwrap();

        let saved = block_metadata_writes(tmp.path(), &v2.id);
        let result = reg.activate(&v1.id).await;
        restore_metadata(tmp.path(), &v2.id, &saved);

        assert!(result.is_err(), "v2 could not be deactivated: {result:?}");
        // v1 was not activated, so there is still one active version.
        let in_memory = active_flags(&reg, "m").await;
        assert_eq!(in_memory, [(1, false), (2, true)]);
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let reopened = ModelRegistry::new(store).await;
        assert_eq!(active_flags(&reopened, "m").await, in_memory);
    }

    /// The version to activate cannot be written: the version that was
    /// active stays active, through `activate` and through `rollback`.
    #[tokio::test]
    async fn test_failed_activation_keeps_the_active_version() {
        let (reg, tmp) = make_registry().await;
        let v1 = reg
            .upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();
        reg.upload("m".into(), xgboost_json("2"), None)
            .await
            .unwrap();

        let saved = block_metadata_writes(tmp.path(), &v1.id);
        let activate = reg.activate(&v1.id).await;
        let rollback = reg.rollback("m").await;
        restore_metadata(tmp.path(), &v1.id, &saved);

        assert!(activate.is_err(), "v1 could not be activated: {activate:?}");
        assert!(rollback.is_err(), "v1 could not be activated: {rollback:?}");
        let in_memory = active_flags(&reg, "m").await;
        assert_eq!(in_memory, [(1, false), (2, true)]);
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let reopened = ModelRegistry::new(store).await;
        assert_eq!(active_flags(&reopened, "m").await, in_memory);
    }

    /// Two versions are active (builds before the activation fixes could
    /// leave that) and the second cannot be deactivated: the first is active
    /// again afterwards and the uploaded version is gone.
    #[tokio::test]
    async fn test_failed_switch_puts_back_what_it_changed() {
        let (reg, tmp) = make_registry().await;
        let v1 = reg
            .upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();
        let v2 = reg
            .upload("m".into(), xgboost_json("2"), None)
            .await
            .unwrap();
        reg.store().activate_model(&v1.id).await.unwrap();
        assert_eq!(active_flags(&reg, "m").await, [(1, true), (2, true)]);

        let saved = block_metadata_writes(tmp.path(), &v2.id);
        // Activating v1, which is active already, must not end with v1
        // deactivated by the undo.
        let activate = reg.activate(&v1.id).await;
        assert!(
            activate.is_err(),
            "v2 could not be deactivated: {activate:?}"
        );
        assert_eq!(active_flags(&reg, "m").await, [(1, true), (2, true)]);
        let result = reg.upload("m".into(), xgboost_json("3"), None).await;
        restore_metadata(tmp.path(), &v2.id, &saved);

        assert!(result.is_err(), "v2 could not be deactivated: {result:?}");
        let in_memory = active_flags(&reg, "m").await;
        assert_eq!(in_memory, [(1, true), (2, true)]);
        assert_eq!(reg.store().list_models().await.len(), 2);
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let reopened = ModelRegistry::new(store).await;
        assert_eq!(active_flags(&reopened, "m").await, in_memory);
    }

    /// A model the store refuses changes nothing: the active version stays.
    #[tokio::test]
    async fn test_refused_upload_keeps_the_active_version() {
        let (reg, tmp) = make_registry().await;
        reg.upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();

        let unknown_format = reg
            .upload("m".into(), r#"{"unknown": 1}"#.into(), None)
            .await;
        let unknown_hint = reg
            .upload("m".into(), xgboost_json("2"), Some("nonsense".into()))
            .await;

        assert!(unknown_format.is_err() && unknown_hint.is_err());
        let in_memory = active_flags(&reg, "m").await;
        assert_eq!(in_memory, [(1, true)]);
        let directories = std::fs::read_dir(tmp.path()).unwrap().count();
        assert_eq!(directories, 1, "a refused model left a directory");
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let reopened = ModelRegistry::new(store).await;
        assert_eq!(active_flags(&reopened, "m").await, in_memory);
    }

    /// A model that cannot be written to disk changes nothing either.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_upload_that_cannot_be_stored_keeps_the_active_version() {
        use std::os::unix::fs::PermissionsExt;

        let (reg, tmp) = make_registry().await;
        reg.upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();

        // No new version directory can be created.
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = reg.upload("m".into(), xgboost_json("2"), None).await;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        if result.is_ok() {
            // Running as root, which the directory mode does not stop.
            return;
        }

        let in_memory = active_flags(&reg, "m").await;
        assert_eq!(in_memory, [(1, true)]);
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let reopened = ModelRegistry::new(store).await;
        assert_eq!(active_flags(&reopened, "m").await, in_memory);
    }

    /// Poll `change` once and drop it, as the handler of an HTTP request is
    /// dropped when its client disconnects. Then wait for the registry to
    /// show `expected`: a change whose caller has gone still runs to its end.
    async fn abandon_and_expect<F: std::future::Future>(
        reg: &ModelRegistry,
        change: F,
        expected: &[(u32, bool)],
    ) {
        {
            let mut change = std::pin::pin!(change);
            let pending = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(change.as_mut().poll(cx).is_pending())
            })
            .await;
            assert!(pending, "the change was over at its first poll");
            // Dropped here, before it has done anything that is awaited.
        }

        let mut flags = Vec::new();
        for _ in 0..500 {
            flags = active_flags(reg, "m").await;
            if flags == expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(flags, expected);
    }

    #[tokio::test]
    async fn test_changes_complete_after_their_caller_has_gone() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();

        let upload = reg.upload("m".into(), xgboost_json("2"), None);
        abandon_and_expect(&reg, upload, &[(1, false), (2, true)]).await;
        let rollback = reg.rollback("m");
        abandon_and_expect(&reg, rollback, &[(1, true), (2, false)]).await;
        let deactivate = reg.deactivate(&v1.id);
        abandon_and_expect(&reg, deactivate, &[(1, false), (2, false)]).await;
        let activate = reg.activate(&v1.id);
        abandon_and_expect(&reg, activate, &[(1, true), (2, false)]).await;
    }

    /// A change that panics is an `io::Error` for its caller, which the HTTP
    /// handlers answer with a 500 like any other storage error.
    #[tokio::test]
    async fn test_panicking_change_is_an_io_error() {
        let result = run_to_completion::<(), _>(async { panic!("expected by this test") }).await;
        let err = result.unwrap_err();
        assert!(err.downcast_ref::<std::io::Error>().is_some(), "{err:?}");
    }

    /// A model in the store that its name's index does not list is not a
    /// version: activating or deactivating it changes nothing.
    #[tokio::test]
    async fn test_model_outside_the_index_is_not_a_version() {
        let (reg, _tmp) = make_registry().await;
        reg.upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();
        let outside = reg
            .store()
            .add_inactive_model_no_dup_check("m".into(), xgboost_json("2"), None)
            .await
            .unwrap();

        let err = reg.activate(&outside.id).await.unwrap_err().to_string();
        assert!(err.contains("not found"), "{err}");
        assert_eq!(active_flags(&reg, "m").await, [(1, true)]);
        assert_eq!(reg.store().is_active(&outside.id).await, Some(false));

        reg.store().activate_model(&outside.id).await.unwrap();
        let err = reg.deactivate(&outside.id).await.unwrap_err().to_string();
        assert!(err.contains("not found"), "{err}");
        assert_eq!(reg.store().is_active(&outside.id).await, Some(true));
    }

    /// A deactivation waits for a change to the index that is under way.
    #[tokio::test]
    async fn test_deactivate_waits_for_the_index_lock() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("m".into(), xgboost_json("1"), None)
            .await
            .unwrap();

        let index = reg.names.read().await;
        let deactivate = tokio::spawn({
            let (reg, id) = (reg.clone(), v1.id.clone());
            async move { reg.deactivate(&id).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(reg.store().is_active(&v1.id).await, Some(true));

        drop(index);
        deactivate.await.unwrap().unwrap();
        assert_eq!(reg.store().is_active(&v1.id).await, Some(false));
    }

    /// An upload and a rollback at the same time end as if one had run after
    /// the other: v3 is active (rollback first) or v2 is (upload first). They
    /// ended with v1 active, and the new version retired, when the rollback
    /// chose its target before the upload and switched after it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_rollback_and_upload_at_once_end_like_one_after_the_other() {
        let (reg, _tmp) = make_registry().await;
        for round in 0..25 {
            let name = format!("m{round}");
            for version in ["1", "2"] {
                let model = xgboost_json(&format!("{round}-{version}"));
                reg.upload(name.clone(), model, None).await.unwrap();
            }

            let third = xgboost_json(&format!("{round}-3"));
            let (upload, rollback) =
                tokio::join!(reg.upload(name.clone(), third, None), reg.rollback(&name));

            upload.unwrap();
            rollback.unwrap();
            let flags = active_flags(&reg, &name).await;
            let active: Vec<u32> = flags.iter().filter(|f| f.1).map(|f| f.0).collect();
            assert!(active == [2] || active == [3], "round {round}: {flags:?}");
        }
    }

    #[tokio::test]
    async fn test_activate_not_found() {
        let (reg, _tmp) = make_registry().await;
        let result = reg.activate("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_deactivate_not_found() {
        let (reg, _tmp) = make_registry().await;
        let result = reg.deactivate("nonexistent").await;
        assert!(result.is_err());
    }

    // ----- Rollback -----

    #[tokio::test]
    async fn test_rollback_activates_previous() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("rb".into(), xgboost_json("r1"), None)
            .await
            .unwrap();
        let v2 = reg
            .upload("rb".into(), xgboost_json("r2"), None)
            .await
            .unwrap();

        // v2 is active. Rollback.
        let rolled = reg.rollback("rb").await.unwrap();
        assert_eq!(rolled.id, v1.id);
        assert_eq!(rolled.version, 1);
        assert!(rolled.is_active);

        // v2 should be deactivated.
        let v2_model = reg.store().get_model(&v2.id).await.unwrap();
        assert!(!v2_model.active);
    }

    #[tokio::test]
    async fn test_rollback_with_three_versions() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("rb3".into(), xgboost_json("a1"), None)
            .await
            .unwrap();
        let v2 = reg
            .upload("rb3".into(), xgboost_json("a2"), None)
            .await
            .unwrap();
        let _v3 = reg
            .upload("rb3".into(), xgboost_json("a3"), None)
            .await
            .unwrap();

        // v3 active. Rollback -> v2.
        let rolled = reg.rollback("rb3").await.unwrap();
        assert_eq!(rolled.id, v2.id);
        assert_eq!(rolled.version, 2);

        // Rollback again -> v1.
        let rolled2 = reg.rollback("rb3").await.unwrap();
        assert_eq!(rolled2.id, v1.id);
        assert_eq!(rolled2.version, 1);
    }

    #[tokio::test]
    async fn test_rollback_at_v1_fails() {
        let (reg, _tmp) = make_registry().await;
        reg.upload("rbf".into(), xgboost_json("only"), None)
            .await
            .unwrap();

        let result = reg.rollback("rbf").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("need at least 2"));
    }

    #[tokio::test]
    async fn test_rollback_unknown_name() {
        let (reg, _tmp) = make_registry().await;
        let result = reg.rollback("ghost").await;
        assert!(result.is_err());
    }

    // ----- Prove uses active version -----

    #[tokio::test]
    async fn test_get_active_model() {
        let (reg, _tmp) = make_registry().await;
        let _v1 = reg
            .upload("active".into(), xgboost_json("am1"), None)
            .await
            .unwrap();
        let v2 = reg
            .upload("active".into(), xgboost_json("am2"), None)
            .await
            .unwrap();

        let active = reg.get_active_model("active").await.unwrap();
        assert_eq!(active.id, v2.id);
    }

    #[tokio::test]
    async fn test_get_active_model_after_rollback() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("ar".into(), xgboost_json("ar1"), None)
            .await
            .unwrap();
        let _v2 = reg
            .upload("ar".into(), xgboost_json("ar2"), None)
            .await
            .unwrap();

        reg.rollback("ar").await.unwrap();

        let active = reg.get_active_model("ar").await.unwrap();
        assert_eq!(active.id, v1.id);
    }

    #[tokio::test]
    async fn test_no_active_after_deactivate_all() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("noact".into(), xgboost_json("na1"), None)
            .await
            .unwrap();

        reg.deactivate(&v1.id).await.unwrap();

        assert!(reg.get_active_model("noact").await.is_none());
    }

    // ----- Deactivated model rejects prove -----

    #[tokio::test]
    async fn test_deactivated_model_not_in_active() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("rej".into(), xgboost_json("rej1"), None)
            .await
            .unwrap();

        reg.deactivate(&v1.id).await.unwrap();

        // get_model still returns it, but active=false.
        let m = reg.store().get_model(&v1.id).await.unwrap();
        assert!(!m.active);

        // get_active_model returns None.
        assert!(reg.get_active_model("rej").await.is_none());
    }

    // ----- Version info -----

    #[tokio::test]
    async fn test_get_version_info() {
        let (reg, _tmp) = make_registry().await;
        let v1 = reg
            .upload("vi".into(), xgboost_json("vi1"), None)
            .await
            .unwrap();

        let info = reg.get_version_info(&v1.id).await.unwrap();
        assert_eq!(info.version, 1);
        assert_eq!(info.name, "vi");
        assert!(info.is_active);
    }

    #[tokio::test]
    async fn test_get_version_info_not_found() {
        let (reg, _tmp) = make_registry().await;
        assert!(reg.get_version_info("nope").await.is_none());
    }
}
