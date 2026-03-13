//! Multi-model hot-reload registry for the TEE enclave.
//!
//! Manages multiple loaded models identified by unique string IDs, enforcing a
//! configurable capacity limit (`MAX_MODELS` env var, default 5).  Each model
//! tracks its SHA-256 hash, file path, size in bytes, and load timestamp.
//!
//! Thread-safe: the inner state is behind `Arc<RwLock<..>>` so multiple
//! request-handler threads can read concurrently while model load/unload
//! operations acquire an exclusive write lock.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::validation::compute_model_hash;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors that can occur during model registry operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The registry has reached its maximum model capacity.
    CapacityExceeded { max_models: usize },
    /// A model with this ID is already loaded.
    DuplicateId(String),
    /// The requested model ID was not found in the registry.
    NotFound(String),
    /// Failed to read the model file from disk.
    IoError(String),
    /// SHA-256 hash of the loaded bytes did not match the expected hash.
    HashMismatch { expected: String, computed: String },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::CapacityExceeded { max_models } => {
                write!(
                    f,
                    "Registry capacity exceeded: max {} models allowed",
                    max_models
                )
            }
            RegistryError::DuplicateId(id) => {
                write!(f, "Model with ID '{}' is already loaded", id)
            }
            RegistryError::NotFound(id) => {
                write!(f, "No model with ID '{}' found in registry", id)
            }
            RegistryError::IoError(msg) => write!(f, "I/O error: {}", msg),
            RegistryError::HashMismatch { expected, computed } => {
                write!(
                    f,
                    "Model hash mismatch: expected {}, computed {}",
                    expected, computed
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}

// ---------------------------------------------------------------------------
// Model types
// ---------------------------------------------------------------------------

/// A model that has been loaded into the registry.
#[derive(Debug, Clone)]
pub struct LoadedModel {
    /// Unique caller-chosen identifier (e.g. "xgboost-iris-v2").
    pub model_id: String,
    /// Filesystem path from which this model was loaded.
    pub model_path: String,
    /// SHA-256 hex digest of the raw model bytes.
    pub model_hash: String,
    /// Monotonic timestamp when the model was loaded.
    pub loaded_at: Instant,
    /// Size of the raw model file in bytes.
    pub size_bytes: usize,
}

/// Lightweight summary returned by [`ModelRegistry::list_models`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Unique identifier for this model.
    pub model_id: String,
    /// SHA-256 hex digest of the model file contents.
    pub model_hash: String,
    /// Size of the model file in bytes.
    pub size_bytes: usize,
}

// ---------------------------------------------------------------------------
// Registry internals
// ---------------------------------------------------------------------------

/// Inner mutable state protected by `RwLock`.
struct RegistryInner {
    models: HashMap<String, LoadedModel>,
    /// Insertion-order tracking so `default_model` returns the first loaded.
    insertion_order: Vec<String>,
    max_models: usize,
}

// ---------------------------------------------------------------------------
// ModelRegistry
// ---------------------------------------------------------------------------

/// Thread-safe registry of loaded models.
///
/// Clone is cheap (inner state is `Arc`-wrapped).
#[derive(Clone)]
pub struct ModelRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

impl ModelRegistry {
    /// Create a new registry with the given capacity limit.
    ///
    /// If `max_models` is 0 it is clamped to 1.
    pub fn new(max_models: usize) -> Self {
        let max_models = max_models.max(1);
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                models: HashMap::new(),
                insertion_order: Vec::new(),
                max_models,
            })),
        }
    }

    /// Create a registry reading `MAX_MODELS` from the environment, defaulting
    /// to 5 if the variable is unset or cannot be parsed.
    pub fn from_env() -> Self {
        let max = std::env::var("MAX_MODELS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(5);
        Self::new(max)
    }

    /// Load a model from `model_path` and register it under `model_id`.
    ///
    /// Reads the file from disk, computes its SHA-256 hash, and stores the
    /// model.  If `expected_hash` is provided, the computed hash is compared
    /// against it (case-insensitive, optional `0x` prefix accepted).
    ///
    /// Returns `Err` if:
    /// - the registry is at capacity ([`RegistryError::CapacityExceeded`]),
    /// - a model with the same ID already exists ([`RegistryError::DuplicateId`]),
    /// - the file cannot be read ([`RegistryError::IoError`]), or
    /// - the hash does not match `expected_hash` ([`RegistryError::HashMismatch`]).
    pub fn load_model(
        &self,
        model_id: &str,
        model_path: &str,
        expected_hash: Option<&str>,
    ) -> Result<(), RegistryError> {
        let raw_bytes = std::fs::read(model_path)
            .map_err(|e| RegistryError::IoError(format!("{}: {}", model_path, e)))?;

        self.load_model_from_bytes(model_id, model_path, &raw_bytes, expected_hash)
    }

    /// Load a model from raw bytes (useful for testing without the filesystem).
    ///
    /// Same semantics as [`load_model`](Self::load_model) but skips disk I/O.
    pub fn load_model_from_bytes(
        &self,
        model_id: &str,
        model_path: &str,
        raw_bytes: &[u8],
        expected_hash: Option<&str>,
    ) -> Result<(), RegistryError> {
        let computed_hash = compute_model_hash(raw_bytes);

        // Validate hash if an expected value was supplied.
        if let Some(expected) = expected_hash {
            let expected_clean = expected
                .strip_prefix("0x")
                .unwrap_or(expected)
                .to_ascii_lowercase();
            if computed_hash != expected_clean {
                return Err(RegistryError::HashMismatch {
                    expected: expected_clean,
                    computed: computed_hash,
                });
            }
        }

        let mut inner = self
            .inner
            .write()
            .expect("ModelRegistry RwLock poisoned (write)");

        // Check for duplicate ID.
        if inner.models.contains_key(model_id) {
            return Err(RegistryError::DuplicateId(model_id.to_string()));
        }

        // Check capacity.
        if inner.models.len() >= inner.max_models {
            return Err(RegistryError::CapacityExceeded {
                max_models: inner.max_models,
            });
        }

        let loaded = LoadedModel {
            model_id: model_id.to_string(),
            model_path: model_path.to_string(),
            model_hash: computed_hash,
            loaded_at: Instant::now(),
            size_bytes: raw_bytes.len(),
        };

        inner.insertion_order.push(model_id.to_string());
        inner.models.insert(model_id.to_string(), loaded);

        Ok(())
    }

    /// Remove a model from the registry by ID.
    ///
    /// Returns `Err(RegistryError::NotFound)` if no model with `model_id` exists.
    pub fn unload_model(&self, model_id: &str) -> Result<(), RegistryError> {
        let mut inner = self
            .inner
            .write()
            .expect("ModelRegistry RwLock poisoned (write)");

        if inner.models.remove(model_id).is_none() {
            return Err(RegistryError::NotFound(model_id.to_string()));
        }

        inner.insertion_order.retain(|id| id != model_id);
        Ok(())
    }

    /// Look up a loaded model by ID. Returns `None` if not found.
    ///
    /// The returned [`LoadedModel`] is cloned so no lock is held by the caller.
    pub fn get_model(&self, model_id: &str) -> Option<LoadedModel> {
        let inner = self
            .inner
            .read()
            .expect("ModelRegistry RwLock poisoned (read)");
        inner.models.get(model_id).cloned()
    }

    /// List all loaded models as lightweight [`ModelInfo`] summaries.
    ///
    /// The list preserves insertion order.
    pub fn list_models(&self) -> Vec<ModelInfo> {
        let inner = self
            .inner
            .read()
            .expect("ModelRegistry RwLock poisoned (read)");

        inner
            .insertion_order
            .iter()
            .filter_map(|id| {
                inner.models.get(id).map(|m| ModelInfo {
                    model_id: m.model_id.clone(),
                    model_hash: m.model_hash.clone(),
                    size_bytes: m.size_bytes,
                })
            })
            .collect()
    }

    /// Return the first model that was loaded (the "default").
    ///
    /// Returns `None` if the registry is empty.
    pub fn default_model(&self) -> Option<LoadedModel> {
        let inner = self
            .inner
            .read()
            .expect("ModelRegistry RwLock poisoned (read)");

        inner
            .insertion_order
            .first()
            .and_then(|id| inner.models.get(id).cloned())
    }

    /// Number of models currently loaded.
    pub fn len(&self) -> usize {
        let inner = self
            .inner
            .read()
            .expect("ModelRegistry RwLock poisoned (read)");
        inner.models.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Maximum number of models the registry can hold.
    pub fn max_models(&self) -> usize {
        let inner = self
            .inner
            .read()
            .expect("ModelRegistry RwLock poisoned (read)");
        inner.max_models
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: short alias for creating a registry.
    fn make_registry(max: usize) -> ModelRegistry {
        ModelRegistry::new(max)
    }

    /// Produce deterministic, distinct byte sequences for testing.
    fn sample_bytes(tag: &str) -> Vec<u8> {
        format!("model-bytes-{}", tag).into_bytes()
    }

    // -- Test 1: basic load and get ----------------------------------------

    #[test]
    fn test_load_and_get_model() {
        let reg = make_registry(5);
        let bytes = sample_bytes("alpha");
        let expected_hash = compute_model_hash(&bytes);

        reg.load_model_from_bytes("alpha", "/tmp/alpha.json", &bytes, None)
            .expect("load should succeed");

        let model = reg.get_model("alpha").expect("model should be found");
        assert_eq!(model.model_id, "alpha");
        assert_eq!(model.model_path, "/tmp/alpha.json");
        assert_eq!(model.model_hash, expected_hash);
        assert_eq!(model.size_bytes, bytes.len());
    }

    // -- Test 2: unload ----------------------------------------------------

    #[test]
    fn test_unload_model() {
        let reg = make_registry(5);
        let bytes = sample_bytes("beta");

        reg.load_model_from_bytes("beta", "/tmp/beta.json", &bytes, None)
            .unwrap();
        assert_eq!(reg.len(), 1);

        reg.unload_model("beta").expect("unload should succeed");
        assert_eq!(reg.len(), 0);
        assert!(reg.get_model("beta").is_none());
    }

    // -- Test 3: unload nonexistent returns error --------------------------

    #[test]
    fn test_unload_nonexistent() {
        let reg = make_registry(5);
        let err = reg.unload_model("ghost").unwrap_err();
        assert_eq!(err, RegistryError::NotFound("ghost".to_string()));
    }

    // -- Test 4: capacity limit enforcement --------------------------------

    #[test]
    fn test_capacity_limit() {
        let reg = make_registry(2);

        reg.load_model_from_bytes("m1", "/m1", &sample_bytes("1"), None)
            .unwrap();
        reg.load_model_from_bytes("m2", "/m2", &sample_bytes("2"), None)
            .unwrap();

        let err = reg
            .load_model_from_bytes("m3", "/m3", &sample_bytes("3"), None)
            .unwrap_err();
        assert_eq!(err, RegistryError::CapacityExceeded { max_models: 2 });

        // After unloading one, loading should succeed again.
        reg.unload_model("m1").unwrap();
        reg.load_model_from_bytes("m3", "/m3", &sample_bytes("3"), None)
            .expect("should succeed after freeing a slot");
        assert_eq!(reg.len(), 2);
    }

    // -- Test 5: list models -----------------------------------------------

    #[test]
    fn test_list_models() {
        let reg = make_registry(5);

        reg.load_model_from_bytes("a", "/a", &sample_bytes("a"), None)
            .unwrap();
        reg.load_model_from_bytes("b", "/b", &sample_bytes("b"), None)
            .unwrap();
        reg.load_model_from_bytes("c", "/c", &sample_bytes("c"), None)
            .unwrap();

        let list = reg.list_models();
        assert_eq!(list.len(), 3);

        // Insertion order is preserved.
        assert_eq!(list[0].model_id, "a");
        assert_eq!(list[1].model_id, "b");
        assert_eq!(list[2].model_id, "c");

        // Hashes and sizes are correct.
        assert_eq!(list[0].model_hash, compute_model_hash(&sample_bytes("a")));
        assert_eq!(list[1].size_bytes, sample_bytes("b").len());
    }

    // -- Test 6: default model ---------------------------------------------

    #[test]
    fn test_default_model() {
        let reg = make_registry(5);

        // Empty registry has no default.
        assert!(reg.default_model().is_none());

        reg.load_model_from_bytes("first", "/first", &sample_bytes("first"), None)
            .unwrap();
        reg.load_model_from_bytes("second", "/second", &sample_bytes("second"), None)
            .unwrap();

        let default = reg.default_model().expect("should have a default");
        assert_eq!(default.model_id, "first");

        // Unloading the first makes the second the new default.
        reg.unload_model("first").unwrap();
        let default = reg.default_model().expect("should still have a default");
        assert_eq!(default.model_id, "second");
    }

    // -- Test 7: duplicate ID rejected -------------------------------------

    #[test]
    fn test_duplicate_id_rejected() {
        let reg = make_registry(5);
        let bytes = sample_bytes("dup");

        reg.load_model_from_bytes("dup", "/dup", &bytes, None)
            .unwrap();

        let err = reg
            .load_model_from_bytes("dup", "/dup2", &bytes, None)
            .unwrap_err();
        assert_eq!(err, RegistryError::DuplicateId("dup".to_string()));
    }

    // -- Test 8: hash validation (pass) ------------------------------------

    #[test]
    fn test_hash_validation_pass() {
        let reg = make_registry(5);
        let bytes = sample_bytes("hv");
        let hash = compute_model_hash(&bytes);

        // Exact match.
        reg.load_model_from_bytes("hv1", "/hv1", &bytes, Some(&hash))
            .expect("exact hash should pass");

        // With 0x prefix.
        reg.load_model_from_bytes("hv2", "/hv2", &bytes, Some(&format!("0x{}", hash)))
            .expect("0x-prefixed hash should pass");

        // Uppercase.
        reg.load_model_from_bytes("hv3", "/hv3", &bytes, Some(&hash.to_uppercase()))
            .expect("uppercase hash should pass");
    }

    // -- Test 9: hash validation (fail) ------------------------------------

    #[test]
    fn test_hash_validation_fail() {
        let reg = make_registry(5);
        let bytes = sample_bytes("bad");
        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";

        let err = reg
            .load_model_from_bytes("bad", "/bad", &bytes, Some(wrong))
            .unwrap_err();
        match err {
            RegistryError::HashMismatch { expected, computed } => {
                assert_eq!(expected, wrong);
                assert_eq!(computed, compute_model_hash(&bytes));
            }
            other => panic!("Expected HashMismatch, got {:?}", other),
        }
    }

    // -- Test 10: thread safety (concurrent reads) -------------------------

    #[test]
    fn test_concurrent_reads() {
        let reg = make_registry(5);
        reg.load_model_from_bytes("shared", "/shared", &sample_bytes("shared"), None)
            .unwrap();

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let r = reg.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        let m = r.get_model("shared");
                        assert!(m.is_some());
                        let list = r.list_models();
                        assert_eq!(list.len(), 1);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread should not panic");
        }
    }

    // -- Test 11: from_env defaults ----------------------------------------

    #[test]
    fn test_from_env_default() {
        // When MAX_MODELS is not set (or set to something valid), from_env
        // should not panic and should produce a reasonable result.
        let reg = ModelRegistry::from_env();
        assert!(reg.max_models() >= 1);
    }

    // -- Test 12: max_models clamped to 1 ----------------------------------

    #[test]
    fn test_max_models_clamped() {
        let reg = make_registry(0);
        assert_eq!(reg.max_models(), 1);
    }

    // -- Test 13: is_empty -------------------------------------------------

    #[test]
    fn test_is_empty() {
        let reg = make_registry(3);
        assert!(reg.is_empty());

        reg.load_model_from_bytes("x", "/x", &sample_bytes("x"), None)
            .unwrap();
        assert!(!reg.is_empty());

        reg.unload_model("x").unwrap();
        assert!(reg.is_empty());
    }

    // -- Test 14: load_model from disk (nonexistent path) ------------------

    #[test]
    fn test_load_model_io_error() {
        let reg = make_registry(5);
        let err = reg
            .load_model("bad", "/tmp/model_registry_tests_nonexistent_xyz.json", None)
            .unwrap_err();
        match err {
            RegistryError::IoError(msg) => {
                assert!(
                    msg.contains("No such file"),
                    "unexpected error message: {}",
                    msg
                );
            }
            other => panic!("expected IoError, got {:?}", other),
        }
    }
}
