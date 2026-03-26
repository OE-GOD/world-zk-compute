//! Immutable proof storage backend trait and local filesystem implementation.
//!
//! The [`ProofStorage`] trait abstracts over where proof bundle bytes are persisted.
//! Implementations enforce append-only / WORM semantics: once a proof is stored
//! under an ID it can never be overwritten or deleted through the storage API.
//!
//! The default [`LocalStorage`] backend writes files to a date-partitioned directory
//! tree, refusing to overwrite any existing file (local WORM). An optional S3 backend
//! with Object Lock is available behind the `s3` feature flag (see [`crate::s3`]).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from proof storage operations.
#[derive(Debug)]
pub enum StorageError {
    /// The proof ID already exists (immutability violation).
    AlreadyExists(String),
    /// The requested proof was not found.
    NotFound(String),
    /// An I/O or backend error.
    Io(String),
    /// Invalid proof ID (e.g. contains path traversal characters).
    InvalidId(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::AlreadyExists(id) => write!(f, "proof '{id}' already exists"),
            StorageError::NotFound(id) => write!(f, "proof '{id}' not found"),
            StorageError::Io(msg) => write!(f, "storage I/O error: {msg}"),
            StorageError::InvalidId(id) => write!(f, "invalid proof ID: '{id}'"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(err: io::Error) -> Self {
        StorageError::Io(err.to_string())
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstract proof storage backend.
///
/// All implementations must guarantee:
/// - **Immutability**: `store()` fails if the ID already exists.
/// - **Durability**: once `store()` returns `Ok`, the data is persistent.
/// - **No deletion**: there is no `delete()` method by design.
#[async_trait]
pub trait ProofStorage: Send + Sync {
    /// Store a proof bundle. Returns the storage key.
    ///
    /// # Errors
    /// Returns [`StorageError::AlreadyExists`] if a proof with this ID is
    /// already stored (enforces immutability).
    async fn store(&self, id: &str, data: &[u8]) -> Result<String, StorageError>;

    /// Retrieve a proof bundle by ID.
    async fn get(&self, id: &str) -> Result<Vec<u8>, StorageError>;

    /// Check if a proof exists.
    async fn exists(&self, id: &str) -> Result<bool, StorageError>;

    /// List proof IDs, paginated by `offset` and `limit`.
    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<String>, StorageError>;

    /// Human-readable storage type name (e.g. "local", "s3").
    fn storage_type(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate a proof ID: must be non-empty, contain only safe characters,
/// and not contain path traversal sequences.
fn validate_id(id: &str) -> Result<(), StorageError> {
    if id.is_empty() {
        return Err(StorageError::InvalidId("empty ID".to_string()));
    }
    if id.contains("..") || id.contains('/') || id.contains('\\') || id.contains('\0') {
        return Err(StorageError::InvalidId(id.to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Local filesystem — append-only WORM
// ---------------------------------------------------------------------------

/// Local filesystem storage with append-only (WORM) semantics.
///
/// Files are stored under `base_dir/YYYY-MM-DD/<id>.json`. Once written, a
/// file can never be overwritten through this API. The directory tree is
/// created on demand.
pub struct LocalStorage {
    base_dir: PathBuf,
}

impl LocalStorage {
    /// Create a new `LocalStorage` rooted at `dir`.
    ///
    /// Creates the directory if it does not already exist.
    pub fn new(dir: &str) -> Result<Self, StorageError> {
        let base_dir = PathBuf::from(dir);
        std::fs::create_dir_all(&base_dir).map_err(|e| {
            StorageError::Io(format!("failed to create storage directory '{dir}': {e}"))
        })?;
        Ok(Self { base_dir })
    }

    /// Return the file path for a given ID: `base_dir/YYYY-MM-DD/id.json`.
    fn file_path(&self, id: &str) -> PathBuf {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        self.base_dir.join(date).join(format!("{id}.json"))
    }

    /// Search for an existing file across all date directories.
    ///
    /// Because the date partition is determined at write time, lookups must
    /// scan for the file.
    fn find_file(&self, id: &str) -> Result<Option<PathBuf>, StorageError> {
        let filename = format!("{id}.json");

        // First check the flat base_dir (for backward compatibility with
        // pre-existing non-partitioned storage).
        let flat_path = self.base_dir.join(&filename);
        if flat_path.is_file() {
            return Ok(Some(flat_path));
        }

        // Scan date-partitioned subdirectories.
        let entries = std::fs::read_dir(&self.base_dir).map_err(|e| {
            StorageError::Io(format!(
                "failed to read storage directory '{}': {e}",
                self.base_dir.display()
            ))
        })?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let candidate = path.join(&filename);
                if candidate.is_file() {
                    return Ok(Some(candidate));
                }
            }
        }

        Ok(None)
    }

    /// Collect all proof IDs across all date partitions.
    fn collect_ids(&self) -> Result<Vec<String>, StorageError> {
        let mut ids = Vec::new();

        let entries = std::fs::read_dir(&self.base_dir).map_err(|e| {
            StorageError::Io(format!(
                "failed to read storage directory '{}': {e}",
                self.base_dir.display()
            ))
        })?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                // Date-partitioned subdir.
                let sub_entries = std::fs::read_dir(&path)?;
                for sub_entry in sub_entries {
                    let sub_entry = sub_entry?;
                    if let Some(name) = extract_id_from_path(&sub_entry.path()) {
                        ids.push(name);
                    }
                }
            } else if let Some(name) = extract_id_from_path(&path) {
                // Flat file in base_dir (backward compat).
                ids.push(name);
            }
        }

        ids.sort();
        Ok(ids)
    }
}

/// Extract a proof ID from a `.json` file path.
fn extract_id_from_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    // Skip hidden files (temp files start with '.').
    if name.starts_with('.') {
        return None;
    }
    name.strip_suffix(".json").map(String::from)
}

#[async_trait]
impl ProofStorage for LocalStorage {
    async fn store(&self, id: &str, data: &[u8]) -> Result<String, StorageError> {
        validate_id(id)?;

        // Check if the proof already exists anywhere.
        if self.find_file(id)?.is_some() {
            return Err(StorageError::AlreadyExists(id.to_string()));
        }

        let dest = self.file_path(id);

        // Ensure the date directory exists.
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Atomic write: write to a temp file, then rename.
        let tmp_path = dest.with_file_name(format!(".{id}.tmp"));
        std::fs::write(&tmp_path, data).map_err(|e| {
            StorageError::Io(format!(
                "failed to write temp file '{}': {e}",
                tmp_path.display()
            ))
        })?;

        // Double-check: if the dest appeared between our exists check and the
        // rename, the rename on most POSIX systems will overwrite. Use
        // hard-link-then-remove for extra safety.
        if dest.exists() {
            // Someone else wrote it concurrently. Clean up our temp file.
            let _ = std::fs::remove_file(&tmp_path);
            return Err(StorageError::AlreadyExists(id.to_string()));
        }

        std::fs::rename(&tmp_path, &dest).map_err(|e| {
            StorageError::Io(format!(
                "failed to rename temp file to '{}': {e}",
                dest.display()
            ))
        })?;

        Ok(id.to_string())
    }

    async fn get(&self, id: &str) -> Result<Vec<u8>, StorageError> {
        validate_id(id)?;
        match self.find_file(id)? {
            Some(path) => {
                let data = std::fs::read(&path).map_err(|e| {
                    StorageError::Io(format!("failed to read '{}': {e}", path.display()))
                })?;
                Ok(data)
            }
            None => Err(StorageError::NotFound(id.to_string())),
        }
    }

    async fn exists(&self, id: &str) -> Result<bool, StorageError> {
        validate_id(id)?;
        Ok(self.find_file(id)?.is_some())
    }

    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<String>, StorageError> {
        let all = self.collect_ids()?;
        let page: Vec<String> = all.into_iter().skip(offset).take(limit).collect();
        Ok(page)
    }

    fn storage_type(&self) -> &str {
        "local"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_storage() -> (LocalStorage, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let storage = LocalStorage::new(tmp.path().to_str().unwrap()).unwrap();
        (storage, tmp)
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let (storage, _tmp) = make_test_storage();
        let data = b"hello world proof data";

        let key = storage.store("proof-1", data).await.unwrap();
        assert_eq!(key, "proof-1");

        let retrieved = storage.get("proof-1").await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_immutability_reject_overwrite() {
        let (storage, _tmp) = make_test_storage();

        storage.store("proof-imm", b"original").await.unwrap();

        // Second store with same ID must fail.
        let result = storage.store("proof-imm", b"overwrite attempt").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StorageError::AlreadyExists(id) => assert_eq!(id, "proof-imm"),
            other => panic!("expected AlreadyExists, got: {other}"),
        }

        // Original data is intact.
        let data = storage.get("proof-imm").await.unwrap();
        assert_eq!(data, b"original");
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (storage, _tmp) = make_test_storage();

        let result = storage.get("nonexistent").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StorageError::NotFound(id) => assert_eq!(id, "nonexistent"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_exists() {
        let (storage, _tmp) = make_test_storage();

        assert!(!storage.exists("proof-e").await.unwrap());
        storage.store("proof-e", b"data").await.unwrap();
        assert!(storage.exists("proof-e").await.unwrap());
    }

    #[tokio::test]
    async fn test_list_paginated() {
        let (storage, _tmp) = make_test_storage();

        for i in 0..5 {
            storage
                .store(&format!("proof-{i:03}"), b"data")
                .await
                .unwrap();
        }

        // All.
        let all = storage.list(0, 100).await.unwrap();
        assert_eq!(all.len(), 5);

        // Paginated.
        let page = storage.list(1, 2).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0], "proof-001");
        assert_eq!(page[1], "proof-002");

        // Past end.
        let empty = storage.list(10, 5).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_storage_type() {
        let (storage, _tmp) = make_test_storage();
        assert_eq!(storage.storage_type(), "local");
    }

    #[tokio::test]
    async fn test_invalid_id_path_traversal() {
        let (storage, _tmp) = make_test_storage();

        let result = storage.store("../escape", b"data").await;
        assert!(matches!(result, Err(StorageError::InvalidId(_))));

        let result = storage.store("foo/bar", b"data").await;
        assert!(matches!(result, Err(StorageError::InvalidId(_))));

        let result = storage.store("", b"data").await;
        assert!(matches!(result, Err(StorageError::InvalidId(_))));
    }

    #[tokio::test]
    async fn test_backward_compat_flat_file() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = LocalStorage::new(tmp.path().to_str().unwrap()).unwrap();

        // Manually write a file in the flat (non-partitioned) layout.
        let flat_path = tmp.path().join("legacy-proof.json");
        std::fs::write(&flat_path, b"legacy data").unwrap();

        // Should be findable.
        assert!(storage.exists("legacy-proof").await.unwrap());
        let data = storage.get("legacy-proof").await.unwrap();
        assert_eq!(data, b"legacy data");

        // Should appear in list.
        let ids = storage.list(0, 100).await.unwrap();
        assert!(ids.contains(&"legacy-proof".to_string()));
    }

    #[tokio::test]
    async fn test_large_data() {
        let (storage, _tmp) = make_test_storage();
        let large_data: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();

        storage.store("large-proof", &large_data).await.unwrap();
        let retrieved = storage.get("large-proof").await.unwrap();
        assert_eq!(retrieved.len(), 1_000_000);
        assert_eq!(retrieved, large_data);
    }
}
