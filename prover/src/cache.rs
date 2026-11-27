//! ELF program caching for faster proof generation
//!
//! Caches downloaded program ELFs to avoid re-downloading and re-uploading.

use alloy::primitives::B256;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use tracing::{debug, info};

/// Write `data` to a temporary file next to `path` and rename it into place,
/// so that `path` never holds a partially written file.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp_path = PathBuf::from(tmp_name);

    let result = std::fs::write(&tmp_path, data).and_then(|_| std::fs::rename(&tmp_path, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

/// In-memory + disk cache for program ELFs
pub struct ProgramCache {
    /// In-memory cache for hot programs
    memory_cache: RwLock<HashMap<B256, Vec<u8>>>,
    /// Disk cache directory
    cache_dir: PathBuf,
    /// Maximum memory cache size in bytes
    max_memory_bytes: usize,
    /// Current memory usage
    current_memory_bytes: RwLock<usize>,
}

impl ProgramCache {
    /// Create a new program cache
    pub fn new(cache_dir: PathBuf, max_memory_mb: usize) -> std::io::Result<Self> {
        // Create cache directory if it doesn't exist
        std::fs::create_dir_all(&cache_dir)?;

        Ok(Self {
            memory_cache: RwLock::new(HashMap::new()),
            cache_dir,
            max_memory_bytes: max_memory_mb * 1024 * 1024,
            current_memory_bytes: RwLock::new(0),
        })
    }

    /// Get a program from cache
    pub fn get(&self, image_id: &B256) -> Option<Vec<u8>> {
        // Check memory cache first (fast path)
        {
            let cache = self.memory_cache.read().ok()?;
            if let Some(elf) = cache.get(image_id) {
                debug!("Cache hit (memory): {}", image_id);
                return Some(elf.clone());
            }
        }

        // Check disk cache (slower path)
        let disk_path = self.disk_path(image_id);
        if disk_path.exists() {
            match std::fs::read(&disk_path) {
                Ok(elf) => {
                    debug!("Cache hit (disk): {}", image_id);
                    // Promote to memory cache if space available
                    self.try_add_to_memory(image_id, &elf);
                    return Some(elf);
                }
                Err(e) => {
                    debug!("Failed to read disk cache: {}", e);
                }
            }
        }

        debug!("Cache miss: {}", image_id);
        None
    }

    /// Store a program in cache
    pub fn put(&self, image_id: &B256, elf: &[u8]) {
        // Always write to disk
        let disk_path = self.disk_path(image_id);
        if let Err(e) = write_atomic(&disk_path, elf) {
            debug!("Failed to write disk cache: {}", e);
        } else {
            info!("Cached program to disk: {}", image_id);
        }

        // Try to add to memory cache
        self.try_add_to_memory(image_id, elf);
    }

    /// Try to add to memory cache if space available
    fn try_add_to_memory(&self, image_id: &B256, elf: &[u8]) {
        let elf_size = elf.len();

        let mut current = self
            .current_memory_bytes
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if *current + elf_size <= self.max_memory_bytes {
            let mut cache = self.memory_cache.write().unwrap_or_else(|e| e.into_inner());
            if !cache.contains_key(image_id) {
                cache.insert(*image_id, elf.to_vec());
                *current += elf_size;
                debug!("Added to memory cache: {} ({} bytes)", image_id, elf_size);
            }
        }
    }

    /// Get disk cache path for an image ID
    fn disk_path(&self, image_id: &B256) -> PathBuf {
        self.cache_dir
            .join(format!("{}.elf", hex::encode(image_id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_cache_put_get() {
        let dir = tempdir().unwrap();
        let cache = ProgramCache::new(dir.path().to_path_buf(), 10).unwrap();

        let image_id = B256::repeat_byte(0x42);
        let elf = vec![0x7f, 0x45, 0x4c, 0x46]; // ELF magic

        cache.put(&image_id, &elf);

        let retrieved = cache.get(&image_id);
        assert_eq!(retrieved, Some(elf));
    }

    #[test]
    fn test_cache_miss() {
        let dir = tempdir().unwrap();
        let cache = ProgramCache::new(dir.path().to_path_buf(), 10).unwrap();

        let image_id = B256::repeat_byte(0x42);
        assert!(cache.get(&image_id).is_none());
    }

    #[test]
    fn test_put_replaces_disk_file_instead_of_overwriting_it() {
        let dir = tempdir().unwrap();
        let cache = ProgramCache::new(dir.path().to_path_buf(), 10).unwrap();

        // An older file for the same image ID, also reachable under a second name
        let image_id = B256::repeat_byte(0x42);
        let disk_path = cache.disk_path(&image_id);
        std::fs::write(&disk_path, b"old contents").unwrap();
        let old_file = dir.path().join("old-file");
        std::fs::hard_link(&disk_path, &old_file).unwrap();

        cache.put(&image_id, b"new contents");

        // The cache path points to a new, complete file; the old file was
        // not written to, so a reader of it never sees a partial ELF.
        assert_eq!(std::fs::read(&disk_path).unwrap(), b"new contents");
        assert_eq!(std::fs::read(&old_file).unwrap(), b"old contents");

        // No temporary file is left behind
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![disk_path.file_name().unwrap(), "old-file".as_ref()]
        );
    }
}
