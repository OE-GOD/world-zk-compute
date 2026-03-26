//! Append-only proof archive storage.
//!
//! Archives proof bundles in a date-partitioned directory structure:
//! `$PROOFS_DIR/YYYY-MM-DD/proof-{uuid}.json`
//!
//! Each proof file contains the inference result, attestation, and
//! metadata needed to reproduce the verification.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Entry stored in the proof archive.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProofArchiveEntry {
    /// Unique identifier for this proof entry.
    pub id: String,
    /// ISO-8601 timestamp when the proof was archived.
    pub archived_at: String,
    /// Result ID from on-chain submission (hex).
    pub result_id: String,
    /// Model hash (hex).
    pub model_hash: String,
    /// Input hash (hex).
    pub input_hash: String,
    /// Result hash (hex).
    pub result_hash: String,
    /// Raw inference result (hex).
    #[serde(default)]
    pub result: String,
    /// ECDSA attestation (hex).
    #[serde(default)]
    pub attestation: String,
    /// Input features used for inference.
    #[serde(default)]
    pub features: Vec<f64>,
    /// Chain ID.
    #[serde(default)]
    pub chain_id: u64,
    /// Enclave address.
    #[serde(default)]
    pub enclave_address: String,
    /// Path to ZK proof file (if generated).
    #[serde(default)]
    pub proof_path: Option<String>,
    /// Whether a dispute was filed.
    #[serde(default)]
    pub disputed: bool,
    /// Whether the result was finalized.
    #[serde(default)]
    pub finalized: bool,
}

/// Append-only proof archive.
pub struct ProofArchive {
    base_dir: PathBuf,
}

impl ProofArchive {
    /// Create a new proof archive rooted at `base_dir`.
    ///
    /// The directory is created if it does not exist.
    pub async fn new(base_dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&base_dir).await?;
        Ok(Self { base_dir })
    }

    /// Archive a proof entry. Returns the path to the created file.
    ///
    /// Files are stored in `$base_dir/YYYY-MM-DD/proof-{uuid}.json`.
    pub async fn store(&self, entry: &ProofArchiveEntry) -> std::io::Result<PathBuf> {
        let date_dir = self.base_dir.join(&today_date_string());
        tokio::fs::create_dir_all(&date_dir).await?;

        let filename = format!("proof-{}.json", &entry.id);
        let path = date_dir.join(&filename);

        let json = serde_json::to_string_pretty(entry).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;

        // Write atomically: write to temp, then rename
        let tmp_path = date_dir.join(format!(".proof-{}.json.tmp", &entry.id));
        tokio::fs::write(&tmp_path, json.as_bytes()).await?;
        tokio::fs::rename(&tmp_path, &path).await?;

        tracing::info!(
            path = %path.display(),
            result_id = %entry.result_id,
            "Archived proof"
        );

        Ok(path)
    }

    /// List all archived proof files, newest first.
    pub async fn list(&self, limit: usize) -> std::io::Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        let mut date_dirs = list_date_dirs(&self.base_dir).await?;
        // Sort descending (newest first)
        date_dirs.sort_unstable_by(|a, b| b.cmp(a));

        for dir in date_dirs {
            let mut files = list_proof_files(&dir).await?;
            files.sort_unstable_by(|a, b| b.cmp(a));
            for file in files {
                entries.push(file);
                if entries.len() >= limit {
                    return Ok(entries);
                }
            }
        }

        Ok(entries)
    }

    /// Read a proof entry by its path.
    pub async fn read(&self, path: impl AsRef<Path>) -> std::io::Result<ProofArchiveEntry> {
        let data = tokio::fs::read_to_string(path).await?;
        serde_json::from_str(&data).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })
    }

    /// Count total archived proofs.
    pub async fn count(&self) -> std::io::Result<usize> {
        let mut total = 0;
        let date_dirs = list_date_dirs(&self.base_dir).await?;
        for dir in date_dirs {
            let files = list_proof_files(&dir).await?;
            total += files.len();
        }
        Ok(total)
    }

    /// Return the base directory path.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

/// Get today's date as YYYY-MM-DD string.
fn today_date_string() -> String {
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple date calculation (no chrono dependency)
    let days_since_epoch = now / 86400;
    epoch_days_to_date(days_since_epoch)
}

/// Convert days since Unix epoch to YYYY-MM-DD.
fn epoch_days_to_date(days: u64) -> String {
    // Adapted from Howard Hinnant's civil_from_days algorithm
    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// List subdirectories matching YYYY-MM-DD pattern.
async fn list_date_dirs(base: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    let mut entries = tokio::fs::read_dir(base).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                // Check YYYY-MM-DD pattern (10 chars, 2 dashes)
                if name.len() == 10 && name.chars().filter(|c| *c == '-').count() == 2 {
                    dirs.push(path);
                }
            }
        }
    }
    Ok(dirs)
}

/// List proof JSON files in a directory.
async fn list_proof_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("proof-") && name.ends_with(".json") && !name.starts_with('.') {
                files.push(path);
            }
        }
    }
    Ok(files)
}

/// Generate a new UUID v4-like identifier (no external dependency).
pub fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.as_nanos();
    // Mix timestamp with a simple counter for uniqueness
    let hash = {
        let mut h: u128 = nanos;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51afd7ed558ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
        h ^= h >> 33;
        h
    };
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (hash >> 96) as u32,
        (hash >> 80) as u16 & 0xffff,
        (hash >> 64) as u16 & 0x0fff,
        ((hash >> 48) as u16 & 0x3fff) | 0x8000,
        hash as u64 & 0xffffffffffff,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epoch_days_to_date() {
        assert_eq!(epoch_days_to_date(0), "1970-01-01");
        assert_eq!(epoch_days_to_date(365), "1971-01-01");
        assert_eq!(epoch_days_to_date(19073), "2022-03-22");
        // 2026-03-23 = day 20535
        assert_eq!(epoch_days_to_date(20535), "2026-03-23");
    }

    #[test]
    fn test_generate_id_format() {
        let id = generate_id();
        // UUID v4 format: 8-4-4-4-12
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn test_today_date_string() {
        let date = today_date_string();
        assert_eq!(date.len(), 10);
        assert!(date.starts_with("20")); // Year 20xx
    }

    #[tokio::test]
    async fn test_archive_store_and_read() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();

        let entry = ProofArchiveEntry {
            id: generate_id(),
            archived_at: "2026-03-23T12:00:00Z".to_string(),
            result_id: "0xabc123".to_string(),
            model_hash: "0xdef456".to_string(),
            input_hash: "0x789".to_string(),
            result_hash: "0x101112".to_string(),
            result: "0x5b302e38355d".to_string(),
            attestation: "0xsig".to_string(),
            features: vec![5.1, 3.5, 1.4, 0.2],
            chain_id: 421614,
            enclave_address: "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65".to_string(),
            proof_path: None,
            disputed: false,
            finalized: false,
        };

        let path = archive.store(&entry).await.unwrap();
        assert!(path.exists());

        let loaded = archive.read(&path).await.unwrap();
        assert_eq!(loaded.result_id, "0xabc123");
        assert_eq!(loaded.features, vec![5.1, 3.5, 1.4, 0.2]);
    }

    #[tokio::test]
    async fn test_archive_list_and_count() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();

        // Store 3 entries
        for i in 0..3 {
            let entry = ProofArchiveEntry {
                id: format!("test-{}", i),
                archived_at: "2026-03-23T12:00:00Z".to_string(),
                result_id: format!("0x{:04x}", i),
                model_hash: String::new(),
                input_hash: String::new(),
                result_hash: String::new(),
                result: String::new(),
                attestation: String::new(),
                features: vec![],
                chain_id: 0,
                enclave_address: String::new(),
                proof_path: None,
                disputed: false,
                finalized: false,
            };
            archive.store(&entry).await.unwrap();
        }

        assert_eq!(archive.count().await.unwrap(), 3);

        let listed = archive.list(10).await.unwrap();
        assert_eq!(listed.len(), 3);

        // Test limit
        let limited = archive.list(2).await.unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[tokio::test]
    async fn test_archive_atomic_write() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();

        let entry = ProofArchiveEntry {
            id: "atomic-test".to_string(),
            archived_at: "2026-03-23T12:00:00Z".to_string(),
            result_id: "0xatomic".to_string(),
            model_hash: String::new(),
            input_hash: String::new(),
            result_hash: String::new(),
            result: String::new(),
            attestation: String::new(),
            features: vec![],
            chain_id: 0,
            enclave_address: String::new(),
            proof_path: None,
            disputed: false,
            finalized: false,
        };

        let path = archive.store(&entry).await.unwrap();

        // Verify no temp files left behind
        let parent = path.parent().unwrap();
        let mut entries = tokio::fs::read_dir(parent).await.unwrap();
        while let Some(e) = entries.next_entry().await.unwrap() {
            let name = e.file_name().to_string_lossy().to_string();
            assert!(!name.starts_with('.'), "temp file left behind: {}", name);
        }
    }

    /// Create a temporary directory for tests.
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("proof-archive-test-{}-{}", pid, id));
        // Remove any leftover from previous runs
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
