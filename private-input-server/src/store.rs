//! Input Storage
//!
//! File-backed + in-memory storage for private inputs, keyed by request ID.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use tracing::{debug, info, warn};

/// Stored input entry
#[derive(Clone)]
pub struct StoredInput {
    /// Raw input bytes
    pub data: Vec<u8>,
    /// SHA-256 digest of the data
    pub digest: [u8; 32],
}

/// File-backed + in-memory input store
pub struct InputStore {
    /// In-memory cache (request_id -> input)
    memory: RwLock<HashMap<u64, StoredInput>>,
    /// Optional directory for persistent storage
    storage_dir: Option<PathBuf>,
}

impl InputStore {
    /// Create a new in-memory store
    pub fn new() -> Self {
        Self {
            memory: RwLock::new(HashMap::new()),
            storage_dir: None,
        }
    }

    /// Create a store with file-backed persistence
    pub fn with_storage_dir(dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            memory: RwLock::new(HashMap::new()),
            storage_dir: Some(dir),
        })
    }

    /// Store input data for a request ID
    pub fn put(&self, request_id: u64, data: Vec<u8>) -> [u8; 32] {
        let digest = sha256(&data);

        // Persist to disk if configured
        if let Some(ref dir) = self.storage_dir {
            let path = dir.join(format!("{}.bin", request_id));
            if let Err(e) = std::fs::write(&path, &data) {
                warn!("Failed to persist input {} to disk: {}", request_id, e);
            } else {
                debug!("Persisted input {} to {:?}", request_id, path);
            }
        }

        // Store in memory
        let stored = StoredInput { data, digest };
        self.memory.write().unwrap().insert(request_id, stored);

        info!(
            "Stored input for request {}: digest={}",
            request_id,
            hex::encode(digest)
        );
        digest
    }

    /// Get input data for a request ID
    pub fn get(&self, request_id: u64) -> Option<StoredInput> {
        // Try memory first
        if let Some(stored) = self.memory.read().unwrap().get(&request_id) {
            return Some(stored.clone());
        }

        // Try disk fallback
        if let Some(ref dir) = self.storage_dir {
            let path = dir.join(format!("{}.bin", request_id));
            if let Ok(data) = std::fs::read(&path) {
                let digest = sha256(&data);
                let stored = StoredInput { data, digest };
                // Cache in memory
                self.memory
                    .write()
                    .unwrap()
                    .insert(request_id, stored.clone());
                return Some(stored);
            }
        }

        None
    }

    /// Remove input for a request ID
    pub fn remove(&self, request_id: u64) -> bool {
        let removed = self.memory.write().unwrap().remove(&request_id).is_some();

        if let Some(ref dir) = self.storage_dir {
            let path = dir.join(format!("{}.bin", request_id));
            let _ = std::fs::remove_file(&path);
        }

        removed
    }

    /// Number of stored inputs
    pub fn len(&self) -> usize {
        self.memory.read().unwrap().len()
    }
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_and_get() {
        let store = InputStore::new();
        let data = b"hello world".to_vec();

        let digest = store.put(42, data.clone());
        assert_ne!(digest, [0u8; 32]);

        let stored = store.get(42).unwrap();
        assert_eq!(stored.data, data);
        assert_eq!(stored.digest, digest);
    }

    #[test]
    fn test_get_missing() {
        let store = InputStore::new();
        assert!(store.get(99).is_none());
    }

    #[test]
    fn test_remove() {
        let store = InputStore::new();
        store.put(42, b"data".to_vec());
        assert!(store.remove(42));
        assert!(store.get(42).is_none());
        assert!(!store.remove(42));
    }

    #[test]
    fn test_file_backed_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();

        let data = b"persistent data".to_vec();
        store.put(1, data.clone());

        // File should exist
        let path = dir.path().join("1.bin");
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), data);

        // Should be retrievable
        let stored = store.get(1).unwrap();
        assert_eq!(stored.data, data);
    }
}
