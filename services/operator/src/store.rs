use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A stored ZK proof ready for on-chain dispute resolution.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct StoredProof {
    pub proof_hex: String,
    pub circuit_hash: String,
    pub public_inputs_hex: String,
    pub gens_hex: String,
}

/// File-based proof storage.
#[allow(dead_code)]
pub struct ProofStore {
    dir: PathBuf,
}

#[allow(dead_code)]
impl ProofStore {
    pub fn new(dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: PathBuf::from(dir),
        })
    }

    /// Save a proof for a given result ID.
    pub fn save(&self, result_id: &str, proof: &StoredProof) -> anyhow::Result<()> {
        let path = self.dir.join(format!("{}.json", result_id));
        let json = serde_json::to_string_pretty(proof)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a proof by result ID, if it exists.
    pub fn load(&self, result_id: &str) -> anyhow::Result<Option<StoredProof>> {
        let path = self.dir.join(format!("{}.json", result_id));
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Check if a proof exists.
    pub fn has(&self, result_id: &str) -> bool {
        self.dir.join(format!("{}.json", result_id)).exists()
    }
}

// ---------------------------------------------------------------------------
// Operator crash-recovery state
// ---------------------------------------------------------------------------

/// Persistent operator state for crash recovery.
///
/// Tracks the last polled block number, active disputes, and processed event
/// IDs so that the operator can resume from where it left off after a restart.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OperatorState {
    /// The next block number to poll from (i.e., all blocks before this have
    /// already been processed).
    pub last_polled_block: u64,

    /// Active disputes keyed by result_id hex string, with value being the
    /// deadline unix timestamp.
    #[serde(default)]
    pub active_disputes: HashMap<String, u64>,

    /// Set of event IDs that have already been processed, used for
    /// deduplication after a restart.  Event IDs are formatted as
    /// `"<tx_hash>:<log_index>"` or similar unique identifiers.
    #[serde(default)]
    pub processed_event_ids: HashSet<String>,
}

/// File-backed state store with atomic writes (write-to-tmp then rename).
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    /// Create a new `StateStore` that persists to `path`.
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into() }
    }

    /// Load state from disk.
    ///
    /// - If the file does not exist, returns `Ok(None)`.
    /// - If the file exists but is corrupt / unparseable, returns `Err`.
    pub fn load(&self) -> anyhow::Result<Option<OperatorState>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(&self.path)
            .map_err(|e| anyhow::anyhow!("Failed to read state file {:?}: {}", self.path, e))?;
        let state: OperatorState = serde_json::from_str(&json)
            .map_err(|e| anyhow::anyhow!("Failed to parse state file {:?}: {}", self.path, e))?;
        Ok(Some(state))
    }

    /// Load state, falling back to `OperatorState::default()` if the file is
    /// missing or corrupt.  Logs a warning on corrupt / missing state.
    pub fn load_or_default(&self) -> OperatorState {
        match self.load() {
            Ok(Some(state)) => {
                tracing::info!(
                    last_polled_block = state.last_polled_block,
                    active_disputes = state.active_disputes.len(),
                    processed_events = state.processed_event_ids.len(),
                    "Loaded operator state from {:?}",
                    self.path
                );
                state
            }
            Ok(None) => {
                tracing::warn!(
                    "State file {:?} not found, starting from latest block",
                    self.path
                );
                OperatorState::default()
            }
            Err(e) => {
                tracing::warn!(
                    "State file {:?} corrupt or unreadable ({}), starting from latest block",
                    self.path,
                    e
                );
                OperatorState::default()
            }
        }
    }

    /// Persist state to disk using atomic write (write to .tmp, then rename).
    ///
    /// This ensures the state file is never left in a half-written state if the
    /// process is killed mid-write.
    pub fn save(&self, state: &OperatorState) -> anyhow::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to create parent directory for {:?}: {}",
                        self.path,
                        e
                    )
                })?;
            }
        }

        let tmp_path = self.tmp_path();
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| anyhow::anyhow!("Failed to serialize operator state: {}", e))?;

        std::fs::write(&tmp_path, &json).map_err(|e| {
            anyhow::anyhow!("Failed to write temp state file {:?}: {}", tmp_path, e)
        })?;

        std::fs::rename(&tmp_path, &self.path).map_err(|e| {
            // Clean up the temp file on rename failure
            let _ = std::fs::remove_file(&tmp_path);
            anyhow::anyhow!(
                "Failed to rename temp state file {:?} -> {:?}: {}",
                tmp_path,
                self.path,
                e
            )
        })?;

        Ok(())
    }

    /// Return the path of the state file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Compute the temp-file path used for atomic writes.
    fn tmp_path(&self) -> PathBuf {
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        PathBuf::from(tmp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_roundtrip() {
        let dir = std::env::temp_dir().join("tee-operator-test-store");
        let _ = std::fs::remove_dir_all(&dir);

        let store = ProofStore::new(dir.to_str().unwrap()).unwrap();

        let proof = StoredProof {
            proof_hex: "0xaabb".to_string(),
            circuit_hash: "0x1234".to_string(),
            public_inputs_hex: "0xccdd".to_string(),
            gens_hex: "0xeeff".to_string(),
        };

        store.save("test-result-1", &proof).unwrap();
        assert!(store.has("test-result-1"));
        assert!(!store.has("nonexistent"));

        let loaded = store.load("test-result-1").unwrap().unwrap();
        assert_eq!(loaded.proof_hex, "0xaabb");
        assert_eq!(loaded.circuit_hash, "0x1234");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ======================== OperatorState tests ========================

    #[test]
    fn test_operator_state_default() {
        let state = OperatorState::default();
        assert_eq!(state.last_polled_block, 0);
        assert!(state.active_disputes.is_empty());
        assert!(state.processed_event_ids.is_empty());
    }

    #[test]
    fn test_operator_state_serde_roundtrip() {
        let state = OperatorState {
            last_polled_block: 12345,
            active_disputes: [
                ("0xaabb".to_string(), 1700000000),
                ("0xccdd".to_string(), 1700086400),
            ]
            .into_iter()
            .collect(),
            processed_event_ids: ["0xdeadbeef:0".to_string(), "0xdeadbeef:1".to_string()]
                .into_iter()
                .collect(),
        };

        let json = serde_json::to_string_pretty(&state).unwrap();
        let loaded: OperatorState = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.last_polled_block, 12345);
        assert_eq!(loaded.active_disputes.len(), 2);
        assert_eq!(loaded.active_disputes.get("0xaabb"), Some(&1700000000u64));
        assert_eq!(loaded.processed_event_ids.len(), 2);
        assert!(loaded.processed_event_ids.contains("0xdeadbeef:0"));
    }

    // ======================== StateStore tests ========================

    #[test]
    fn test_state_store_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("operator-state.json");
        let store = StateStore::new(&state_path);

        let state = OperatorState {
            last_polled_block: 42,
            active_disputes: [("0x1111".to_string(), 9999)].into_iter().collect(),
            processed_event_ids: ["tx1:0".to_string()].into_iter().collect(),
        };

        store.save(&state).unwrap();

        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded, state);
    }

    #[test]
    fn test_state_store_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("nonexistent.json");
        let store = StateStore::new(&state_path);

        let loaded = store.load().unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_state_store_missing_file_load_or_default() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("nonexistent.json");
        let store = StateStore::new(&state_path);

        let state = store.load_or_default();
        assert_eq!(state.last_polled_block, 0);
        assert!(state.active_disputes.is_empty());
        assert!(state.processed_event_ids.is_empty());
    }

    #[test]
    fn test_state_store_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("corrupt.json");
        std::fs::write(&state_path, "this is not valid json {{{").unwrap();

        let store = StateStore::new(&state_path);
        let result = store.load();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Failed to parse state file"),
            "Expected parse error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_state_store_corrupt_file_load_or_default() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("corrupt.json");
        std::fs::write(&state_path, "NOT JSON!!!").unwrap();

        let store = StateStore::new(&state_path);
        let state = store.load_or_default();
        // Should fall back to default
        assert_eq!(state.last_polled_block, 0);
        assert!(state.active_disputes.is_empty());
    }

    #[test]
    fn test_state_store_atomic_write_no_tmp_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let store = StateStore::new(&state_path);

        let state = OperatorState {
            last_polled_block: 100,
            active_disputes: HashMap::new(),
            processed_event_ids: HashSet::new(),
        };

        store.save(&state).unwrap();

        // The .tmp file should not exist after a successful save
        let tmp_path = store.tmp_path();
        assert!(
            !tmp_path.exists(),
            "Temp file {:?} should not exist after successful save",
            tmp_path
        );

        // The actual file should exist
        assert!(state_path.exists());
    }

    #[test]
    fn test_state_store_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let store = StateStore::new(&state_path);

        // Write initial state
        let state1 = OperatorState {
            last_polled_block: 100,
            active_disputes: HashMap::new(),
            processed_event_ids: HashSet::new(),
        };
        store.save(&state1).unwrap();

        // Write updated state
        let state2 = OperatorState {
            last_polled_block: 200,
            active_disputes: {
                let mut m = HashMap::new();
                m.insert("dispute-1".to_string(), 5000);
                m
            },
            processed_event_ids: {
                let mut s = HashSet::new();
                s.insert("evt-1".to_string());
                s
            },
        };
        store.save(&state2).unwrap();

        // Load should return the latest state
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.last_polled_block, 200);
        assert_eq!(loaded.active_disputes.len(), 1);
        assert_eq!(loaded.processed_event_ids.len(), 1);
    }

    #[test]
    fn test_state_store_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nested_path = dir.path().join("sub").join("dir").join("state.json");
        let store = StateStore::new(&nested_path);

        let state = OperatorState {
            last_polled_block: 50,
            active_disputes: HashMap::new(),
            processed_event_ids: HashSet::new(),
        };
        store.save(&state).unwrap();

        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.last_polled_block, 50);
    }

    #[test]
    fn test_processed_event_ids_dedup() {
        let mut state = OperatorState::default();

        // Insert the same event ID multiple times
        state.processed_event_ids.insert("tx1:0".to_string());
        state.processed_event_ids.insert("tx1:0".to_string());
        state.processed_event_ids.insert("tx1:0".to_string());

        // HashSet deduplicates
        assert_eq!(state.processed_event_ids.len(), 1);

        // Different IDs are distinct
        state.processed_event_ids.insert("tx1:1".to_string());
        state.processed_event_ids.insert("tx2:0".to_string());
        assert_eq!(state.processed_event_ids.len(), 3);

        // contains() works for dedup checking
        assert!(state.processed_event_ids.contains("tx1:0"));
        assert!(!state.processed_event_ids.contains("tx3:0"));
    }

    #[test]
    fn test_state_store_path() {
        let store = StateStore::new("/tmp/test-state.json");
        assert_eq!(store.path(), Path::new("/tmp/test-state.json"));
    }

    #[test]
    fn test_operator_state_missing_optional_fields_in_json() {
        // Older state files might not have all fields. The serde defaults
        // should handle this gracefully.
        let json = r#"{"last_polled_block": 999}"#;
        let state: OperatorState = serde_json::from_str(json).unwrap();
        assert_eq!(state.last_polled_block, 999);
        assert!(state.active_disputes.is_empty());
        assert!(state.processed_event_ids.is_empty());
    }
}
