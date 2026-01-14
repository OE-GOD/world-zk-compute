//! ELF program caching for faster proof generation
//!
//! Caches downloaded program ELFs to avoid re-downloading and re-uploading.

#![allow(dead_code)]

use alloy::primitives::B256;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use tracing::{debug, info};

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
        if let Err(e) = std::fs::write(&disk_path, elf) {
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

        let mut current = self.current_memory_bytes.write().unwrap();
        if *current + elf_size <= self.max_memory_bytes {
            let mut cache = self.memory_cache.write().unwrap();
            if !cache.contains_key(image_id) {
                cache.insert(*image_id, elf.to_vec());
                *current += elf_size;
                debug!("Added to memory cache: {} ({} bytes)", image_id, elf_size);
            }
        }
    }

    /// Get disk cache path for an image ID
    fn disk_path(&self, image_id: &B256) -> PathBuf {
        self.cache_dir.join(format!("{}.elf", hex::encode(image_id)))
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let memory_entries = self.memory_cache.read()
            .map(|c| c.len())
            .unwrap_or(0);
        let memory_bytes = *self.current_memory_bytes.read().unwrap();

        // Count disk entries
        let disk_entries = std::fs::read_dir(&self.cache_dir)
            .map(|entries| entries.count())
            .unwrap_or(0);

        CacheStats {
            memory_entries,
            memory_bytes,
            disk_entries,
            max_memory_bytes: self.max_memory_bytes,
        }
    }

    /// Clear the cache
    #[allow(dead_code)]
    pub fn clear(&self) -> std::io::Result<()> {
        // Clear memory
        {
            let mut cache = self.memory_cache.write().unwrap();
            cache.clear();
            *self.current_memory_bytes.write().unwrap() = 0;
        }

        // Clear disk
        for entry in std::fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            if entry.path().extension().map_or(false, |e| e == "elf") {
                std::fs::remove_file(entry.path())?;
            }
        }

        info!("Cache cleared");
        Ok(())
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub memory_entries: usize,
    pub memory_bytes: usize,
    pub disk_entries: usize,
    pub max_memory_bytes: usize,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cache: {} in memory ({:.2} MB / {:.2} MB), {} on disk",
            self.memory_entries,
            self.memory_bytes as f64 / 1024.0 / 1024.0,
            self.max_memory_bytes as f64 / 1024.0 / 1024.0,
            self.disk_entries
        )
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
}
