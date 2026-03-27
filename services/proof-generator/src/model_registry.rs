//! Model registry with versioning, activation controls, and rollback.
//!
//! Built on top of [`ModelStore`] to add:
//!
//! - **Versioning**: Each upload of the same model name creates a new version
//! - **Activation**: Only active model versions can be used for proving
//! - **Rollback**: Quickly revert to a previous version
//!
//! The registry maintains a secondary index from model name to an ordered list
//! of version IDs (oldest-first). Activation state is tracked per version ID
//! and persisted via the underlying `ModelStore`.

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
    /// deactivated.
    ///
    /// Unlike the raw `ModelStore::add_model`, duplicate hashes are allowed
    /// across different names (but still rejected within the same name).
    pub async fn upload(
        &self,
        name: String,
        raw_json: String,
        format_hint: Option<String>,
    ) -> anyhow::Result<VersionInfo> {
        let model_hash = compute_model_hash(&raw_json);

        // Check for duplicate hash within the same model name.
        {
            let names = self.names.read().await;
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
        }

        // Add to the underlying store (bypass its global duplicate check by
        // using our own logic above).
        let meta = self
            .store
            .add_model_no_dup_check(name.clone(), raw_json, format_hint)
            .await?;

        let now = chrono::Utc::now().to_rfc3339();

        // Update the name index and deactivate previous active version.
        let version;
        {
            let mut names = self.names.write().await;
            let entry = names.entry(name.clone()).or_insert_with(|| NameEntry {
                versions: Vec::new(),
            });

            // Deactivate any currently active version.
            for ve in &mut entry.versions {
                if ve.activated_at.is_some() {
                    if let Some(m) = self.store.get_model(&ve.id).await {
                        if m.active {
                            let _ = self.store.deactivate_model(&ve.id).await;
                        }
                    }
                    // Keep the activated_at timestamp for history, but the
                    // store's `active` flag is now false.
                }
            }

            version = entry.versions.len() as u32 + 1;
            entry.versions.push(VersionEntry {
                id: meta.id.clone(),
                version,
                activated_at: Some(now.clone()),
            });
        }

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
    /// Deactivates any other active version of the same model name.
    pub async fn activate(&self, id: &str) -> anyhow::Result<VersionInfo> {
        let model = self
            .store
            .get_model(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", id))?;

        let now = chrono::Utc::now().to_rfc3339();

        // Deactivate all other versions of this name, activate this one.
        {
            let mut names = self.names.write().await;
            let entry = names.get_mut(&model.name).ok_or_else(|| {
                anyhow::anyhow!("Model name not found in registry: {}", model.name)
            })?;

            for ve in &mut entry.versions {
                if ve.id == id {
                    ve.activated_at = Some(now.clone());
                    self.store.activate_model(id).await?;
                } else if let Some(m) = self.store.get_model(&ve.id).await {
                    if m.active {
                        let _ = self.store.deactivate_model(&ve.id).await;
                    }
                }
            }
        }

        let model = self.store.get_model(id).await.unwrap();
        let names = self.names.read().await;
        let entry = names.get(&model.name).unwrap();
        let ve = entry.versions.iter().find(|v| v.id == id).unwrap();

        Ok(VersionInfo {
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

    /// Deactivate a specific model version by its ID.
    pub async fn deactivate(&self, id: &str) -> anyhow::Result<VersionInfo> {
        let model = self
            .store
            .get_model(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", id))?;

        self.store.deactivate_model(id).await?;

        let names = self.names.read().await;
        let entry = names
            .get(&model.name)
            .ok_or_else(|| anyhow::anyhow!("Model name not found in registry"))?;
        let ve = entry
            .versions
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| anyhow::anyhow!("Version entry not found"))?;

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
        let names = self.names.read().await;
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
            if let Some(m) = self.store.get_model(&ve.id).await {
                if m.active {
                    active_idx = Some(i);
                }
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
        drop(names);

        // Activate the previous version (this also deactivates the current).
        self.activate(&prev_id).await
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
