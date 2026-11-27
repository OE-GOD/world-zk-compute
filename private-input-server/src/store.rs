//! Input Storage
//!
//! File-backed + in-memory storage for private inputs, keyed by request ID.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use tracing::{debug, info, warn};

/// Stored input entry
#[derive(Clone)]
#[allow(dead_code)]
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

impl Default for InputStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InputStore {
    /// Create a new in-memory store
    pub fn new() -> Self {
        Self {
            memory: RwLock::new(HashMap::new()),
            storage_dir: None,
        }
    }

    /// Create a store with file-backed persistence. On Unix a directory that
    /// does not exist yet is created accessible to its owner only (0700). An
    /// existing directory keeps its mode: it may be shared, or owned by
    /// another user, so changing it could break other users of it or stop the
    /// server from starting. A warning is logged when group or other users
    /// have any access to it. The input files are created 0600 either way.
    pub fn with_storage_dir(dir: PathBuf) -> anyhow::Result<Self> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir)?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                warn!(
                    "Storage directory {:?} has mode {:o}, so other users can \
                     reach into it; the input files in it are still created 0600",
                    dir, mode
                );
            }
        }
        Ok(Self {
            memory: RwLock::new(HashMap::new()),
            storage_dir: Some(dir),
        })
    }

    /// Store input data for a request ID. With a storage directory the input
    /// is on disk before this returns: the file is synced, and on Unix the
    /// directory too, so that the file's name also survives a power loss.
    /// Elsewhere only the file contents are synced. If the input cannot be
    /// written and synced the error is returned and the new input is not
    /// kept (its file is removed, unless that fails too). An earlier input for
    /// the same request ID is then served only if its file is still on disk.
    /// Nothing orders two puts for the same request ID.
    pub fn put(&self, request_id: u64, data: Vec<u8>) -> std::io::Result<[u8; 32]> {
        let digest = sha256(&data);

        if let Some(ref dir) = self.storage_dir {
            let path = dir.join(format!("{}.bin", request_id));
            if let Err(e) = write_private_file_atomic(&path, &data) {
                warn!("Failed to persist input {} to disk: {}", request_id, e);
                // A failure after the rename removes the file, and with it an
                // earlier input for this request; memory must not serve that
                // input either. After an earlier failure the old file is still
                // there, and get() reads it back.
                self.memory.write().unwrap().remove(&request_id);
                return Err(e);
            }
            debug!("Persisted input {} to {:?}", request_id, path);
        }

        // Store in memory
        let stored = StoredInput { data, digest };
        self.memory.write().unwrap().insert(request_id, stored);

        info!(
            "Stored input for request {}: digest={}",
            request_id,
            hex::encode(digest)
        );
        Ok(digest)
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
    #[allow(dead_code)]
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

    /// Whether the store holds no inputs
    pub fn is_empty(&self) -> bool {
        self.memory.read().unwrap().is_empty()
    }
}

/// Replace `path` with `contents`: write a temporary file in the same
/// directory, sync it and rename it over `path`, so that a crash or a failed
/// write never leaves a truncated input for `get` to serve. On Unix the file
/// is created readable and writable by the owner only, and the directory is
/// synced after the rename. If that sync fails `path` is removed, so that an
/// input reported as not stored is not served later.
fn write_private_file_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    // Each write gets its own temporary name, so two uploads for the same
    // request cannot write into one temporary file.
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = path.with_file_name(name);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let result = options
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return result;
    }

    // The rename changes only the directory. Until the directory is synced a
    // power loss can lose the new name, although the file's data is on disk.
    #[cfg(unix)]
    {
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if let Err(e) = std::fs::File::open(dir).and_then(|d| d.sync_all()) {
            let _ = std::fs::remove_file(path);
            return Err(e);
        }
    }
    Ok(())
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

        let digest = store.put(42, data.clone()).unwrap();
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
        store.put(42, b"data".to_vec()).unwrap();
        assert!(store.remove(42));
        assert!(store.get(42).is_none());
        assert!(!store.remove(42));
    }

    #[test]
    fn test_file_backed_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();

        let data = b"persistent data".to_vec();
        store.put(1, data.clone()).unwrap();

        // File should exist
        let path = dir.path().join("1.bin");
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), data);

        // Should be retrievable
        let stored = store.get(1).unwrap();
        assert_eq!(stored.data, data);
    }

    #[cfg(unix)]
    #[test]
    fn test_new_storage_dir_and_input_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("inputs");
        let store = InputStore::with_storage_dir(dir.clone()).unwrap();
        assert_eq!(mode(&dir), 0o700);

        store.put(7, b"private".to_vec()).unwrap();
        assert_eq!(mode(&dir.join("7.bin")), 0o600);
        // Replacing an input keeps it private and leaves no temporary file.
        store.put(7, b"private, again".to_vec()).unwrap();
        assert_eq!(mode(&dir.join("7.bin")), 0o600);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["7.bin"]);
    }

    /// Collects what the `tracing` macros print while `f` runs.
    fn logs_of(f: impl FnOnce()) -> String {
        use std::sync::{Arc, Mutex};
        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Sink(Arc::new(Mutex::new(Vec::new())));
        let writer = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = sink.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn test_existing_storage_dir_keeps_its_mode_and_is_warned_about_when_shared() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        let logs = logs_of(|| {
            let store = InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();
            store.put(8, b"private".to_vec()).unwrap();
        });
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path()), 0o755);
        assert_eq!(mode(&dir.path().join("8.bin")), 0o600);
        assert!(logs.contains("WARN"), "{logs}");
        assert!(logs.contains("has mode 755"), "{logs}");
    }

    #[cfg(unix)]
    #[test]
    fn test_owner_only_storage_dir_gives_no_warning() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        let logs = logs_of(|| {
            InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();
        });
        assert!(!logs.contains("WARN"), "{logs}");
    }

    #[cfg(unix)]
    #[test]
    fn test_input_is_not_stored_when_its_directory_cannot_be_synced() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("inputs");
        let store = InputStore::with_storage_dir(dir.clone()).unwrap();
        // Write and search but no read: files can still be created and
        // renamed in the directory, but it cannot be opened to sync it.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300)).unwrap();
        if std::fs::File::open(&dir).is_ok() {
            // Root opens it anyway, so the failure cannot be made here.
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            eprintln!("skipped: the directory can be opened without read permission");
            return;
        }

        let result = store.put(9, b"not durable".to_vec());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(result.is_err());
        assert!(store.get(9).is_none());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn test_failed_replacing_upload_does_not_serve_the_earlier_input_from_memory() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("inputs");
        let store = InputStore::with_storage_dir(dir.clone()).unwrap();
        store.put(9, b"first".to_vec()).unwrap();
        // As above: the new file replaces the old one, then the directory
        // cannot be synced, so the new file is removed and no file is left.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300)).unwrap();
        if std::fs::File::open(&dir).is_ok() {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            eprintln!("skipped: the directory can be opened without read permission");
            return;
        }

        let result = store.put(9, b"second".to_vec());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(store.get(9).is_none());
    }

    #[test]
    fn test_failed_write_is_an_error_and_stores_nothing() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("inputs");
        let store = InputStore::with_storage_dir(dir.clone()).unwrap();
        // The directory is gone, so the input cannot be written.
        std::fs::remove_dir(&dir).unwrap();

        assert!(store.put(3, b"lost".to_vec()).is_err());
        assert!(store.get(3).is_none());
        assert!(store.is_empty());
    }
}
