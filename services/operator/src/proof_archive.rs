//! Append-only proof archive storage.
//!
//! Archives proof bundles in a date-partitioned directory structure:
//! `$PROOFS_DIR/YYYY-MM-DD/proof-{uuid}.json`
//!
//! Each proof file contains the inference result, attestation, and
//! metadata needed to reproduce the verification.

use std::io::Write;
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

impl Default for ProofArchiveEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            archived_at: String::new(),
            result_id: String::new(),
            model_hash: String::new(),
            input_hash: String::new(),
            result_hash: String::new(),
            result: String::new(),
            attestation: String::new(),
            features: Vec::new(),
            chain_id: 0,
            enclave_address: String::new(),
            proof_path: None,
            disputed: false,
            finalized: false,
        }
    }
}

/// Append-only proof archive with expiry and rotation support.
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

        let json = serde_json::to_string_pretty(entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

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
        serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
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

    /// Delete proof files older than `max_age_days`.
    ///
    /// Removes entire date directories whose date is older than the cutoff.
    /// Returns the number of files deleted.
    pub async fn expire(&self, max_age_days: u32) -> std::io::Result<usize> {
        let cutoff = {
            let now_secs = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let cutoff_secs = now_secs.saturating_sub(max_age_days as u64 * 86400);
            epoch_days_to_date(cutoff_secs / 86400)
        };

        let mut deleted = 0;
        let date_dirs = list_date_dirs(&self.base_dir).await?;
        for dir in date_dirs {
            if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
                if name < cutoff.as_str() {
                    let files = list_proof_files(&dir).await?;
                    let count = files.len();
                    tokio::fs::remove_dir_all(&dir).await?;
                    tracing::info!(dir = %dir.display(), files = count, "Expired proof directory");
                    deleted += count;
                }
            }
        }

        Ok(deleted)
    }

    /// Rotate: keep only the most recent `max_files` proof files.
    ///
    /// Deletes oldest files first. Returns the number of files deleted.
    pub async fn rotate(&self, max_files: usize) -> std::io::Result<usize> {
        // Collect all files sorted newest-first
        let all_files = self.list(usize::MAX).await?;
        if all_files.len() <= max_files {
            return Ok(0);
        }

        let to_delete = &all_files[max_files..];
        let mut deleted = 0;
        for path in to_delete {
            if tokio::fs::remove_file(path).await.is_ok() {
                deleted += 1;
            }
        }

        // Clean up empty date directories
        let date_dirs = list_date_dirs(&self.base_dir).await?;
        for dir in date_dirs {
            let files = list_proof_files(&dir).await?;
            if files.is_empty() {
                let _ = tokio::fs::remove_dir(&dir).await;
            }
        }

        tracing::info!(deleted, max_files, "Rotated proof archive");
        Ok(deleted)
    }

    /// List all proof files whose date directory is older than `ttl_days`.
    ///
    /// Returns paths sorted oldest-first.
    pub async fn list_expired(&self, ttl_days: u32) -> std::io::Result<Vec<PathBuf>> {
        let cutoff = ttl_cutoff_date(ttl_days);
        let mut expired = Vec::new();

        let date_dirs = list_date_dirs(&self.base_dir).await?;
        for dir in &date_dirs {
            if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
                if name < cutoff.as_str() {
                    let files = list_proof_files(dir).await?;
                    expired.extend(files);
                }
            }
        }

        expired.sort_unstable();
        Ok(expired)
    }

    /// Archive expired proofs to cold storage with gzip compression.
    ///
    /// Proofs older than `ttl_days` are compressed and moved to `archive_dir`,
    /// preserving the date-partitioned directory structure:
    ///   `$archive_dir/YYYY-MM-DD/proof-{id}.json.gz`
    ///
    /// If `dry_run` is true, no files are moved or compressed; the function
    /// only returns the list of files that *would* be archived.
    ///
    /// Returns the list of original file paths that were (or would be) archived.
    pub async fn archive_expired(
        &self,
        ttl_days: u32,
        archive_dir: impl AsRef<Path>,
        dry_run: bool,
    ) -> std::io::Result<Vec<PathBuf>> {
        let archive_dir = archive_dir.as_ref();
        let expired = self.list_expired(ttl_days).await?;

        if expired.is_empty() {
            tracing::info!("No expired proofs to archive");
            return Ok(Vec::new());
        }

        if dry_run {
            tracing::info!(
                count = expired.len(),
                ttl_days,
                "[dry-run] Would archive expired proofs"
            );
            for path in &expired {
                tracing::info!(path = %path.display(), "[dry-run] Would archive");
            }
            return Ok(expired);
        }

        tokio::fs::create_dir_all(archive_dir).await?;

        let mut archived = Vec::new();
        for path in &expired {
            // Determine the date subdirectory name from the parent
            let date_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            let dest_dir = archive_dir.join(date_name);
            tokio::fs::create_dir_all(&dest_dir).await?;

            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("proof.json");
            let dest_path = dest_dir.join(format!("{}.gz", filename));

            // Read, compress, and write
            let data = tokio::fs::read(path).await?;
            let compressed = compress_gzip(&data)?;
            tokio::fs::write(&dest_path, &compressed).await?;

            // Remove original
            tokio::fs::remove_file(path).await?;
            tracing::info!(
                src = %path.display(),
                dest = %dest_path.display(),
                original_size = data.len(),
                compressed_size = compressed.len(),
                "Archived proof to cold storage"
            );
            archived.push(path.clone());
        }

        // Clean up empty date directories in the source
        let date_dirs = list_date_dirs(&self.base_dir).await?;
        for dir in date_dirs {
            let files = list_proof_files(&dir).await?;
            if files.is_empty() {
                let _ = tokio::fs::remove_dir(&dir).await;
            }
        }

        tracing::info!(
            count = archived.len(),
            archive_dir = %archive_dir.display(),
            "Archived expired proofs to cold storage"
        );

        Ok(archived)
    }
}

/// Default proof TTL in days (read from `PROOF_TTL_DAYS` env var).
pub fn default_ttl_days() -> u32 {
    std::env::var("PROOF_TTL_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(365)
}

/// Default cold-storage archive directory (read from `PROOF_ARCHIVE_DIR` env var).
pub fn default_archive_dir() -> Option<String> {
    std::env::var("PROOF_ARCHIVE_DIR")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Compute the cutoff date string for a given TTL.
fn ttl_cutoff_date(ttl_days: u32) -> String {
    let now_secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let cutoff_secs = now_secs.saturating_sub(ttl_days as u64 * 86400);
    epoch_days_to_date(cutoff_secs / 86400)
}

/// Compress data using gzip (default compression level).
fn compress_gzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

/// Decompress gzip data.
pub fn decompress_gzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let mut decoder = GzDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
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

    #[tokio::test]
    async fn test_rotate_keeps_newest() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();

        // Store 5 entries
        for i in 0..5 {
            let entry = ProofArchiveEntry {
                id: format!("rotate-{}", i),
                archived_at: format!("2026-03-23T12:{:02}:00Z", i),
                result_id: format!("0x{:04x}", i),
                ..Default::default()
            };
            archive.store(&entry).await.unwrap();
        }
        assert_eq!(archive.count().await.unwrap(), 5);

        // Rotate to keep only 3
        let deleted = archive.rotate(3).await.unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(archive.count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_rotate_noop_under_limit() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();

        let entry = ProofArchiveEntry {
            id: "single".to_string(),
            ..Default::default()
        };
        archive.store(&entry).await.unwrap();

        let deleted = archive.rotate(100).await.unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(archive.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_expire_removes_old_dirs() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();

        // Create an old date directory manually
        let old_dir = tmp.join("2020-01-01");
        tokio::fs::create_dir_all(&old_dir).await.unwrap();
        tokio::fs::write(old_dir.join("proof-old.json"), "{}")
            .await
            .unwrap();

        // Store a current entry
        let entry = ProofArchiveEntry {
            id: "current".to_string(),
            ..Default::default()
        };
        archive.store(&entry).await.unwrap();

        // Expire anything older than 30 days
        let expired = archive.expire(30).await.unwrap();
        assert_eq!(expired, 1);

        // Current entry should remain
        assert_eq!(archive.count().await.unwrap(), 1);
        // Old dir should be gone
        assert!(!old_dir.exists());
    }

    #[tokio::test]
    async fn test_list_expired_returns_old_proofs() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();

        // Create an old date directory (well past any TTL)
        let old_dir = tmp.join("2020-06-15");
        tokio::fs::create_dir_all(&old_dir).await.unwrap();
        tokio::fs::write(old_dir.join("proof-expired1.json"), r#"{"id":"expired1"}"#)
            .await
            .unwrap();
        tokio::fs::write(old_dir.join("proof-expired2.json"), r#"{"id":"expired2"}"#)
            .await
            .unwrap();

        // Store a current entry (today)
        let entry = ProofArchiveEntry {
            id: "current".to_string(),
            ..Default::default()
        };
        archive.store(&entry).await.unwrap();

        // TTL of 30 days: the 2020 proofs are expired, the current one is not
        let expired = archive.list_expired(30).await.unwrap();
        assert_eq!(expired.len(), 2);
        for path in &expired {
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("proof-expired"));
        }
    }

    #[tokio::test]
    async fn test_proof_within_ttl_stays_in_place() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();
        let cold = tempdir();

        // Store a current entry (today's date)
        let entry = ProofArchiveEntry {
            id: "fresh".to_string(),
            result_id: "0xfresh".to_string(),
            ..Default::default()
        };
        archive.store(&entry).await.unwrap();

        // Archive with 365-day TTL — nothing should move
        let archived = archive.archive_expired(365, &cold, false).await.unwrap();
        assert!(archived.is_empty());
        assert_eq!(archive.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_expired_proof_moves_to_archive() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();
        let cold = tempdir();

        // Create an old date directory with a proof
        let old_dir = tmp.join("2019-01-01");
        tokio::fs::create_dir_all(&old_dir).await.unwrap();
        let old_proof = ProofArchiveEntry {
            id: "old-proof".to_string(),
            result_id: "0xold".to_string(),
            model_hash: "0xmodel".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&old_proof).unwrap();
        tokio::fs::write(old_dir.join("proof-old-proof.json"), json.as_bytes())
            .await
            .unwrap();

        // Store a current proof
        let current = ProofArchiveEntry {
            id: "current".to_string(),
            ..Default::default()
        };
        archive.store(&current).await.unwrap();

        // Archive with 30-day TTL
        let archived = archive.archive_expired(30, &cold, false).await.unwrap();
        assert_eq!(archived.len(), 1);

        // Current proof should remain
        assert_eq!(archive.count().await.unwrap(), 1);

        // Old proof should be gone from source
        assert!(!old_dir.join("proof-old-proof.json").exists());

        // Compressed file should exist in cold storage
        let compressed_path = cold.join("2019-01-01/proof-old-proof.json.gz");
        assert!(compressed_path.exists());

        // Verify the compressed file is valid and contains the original data
        let compressed = tokio::fs::read(&compressed_path).await.unwrap();
        let decompressed = decompress_gzip(&compressed).unwrap();
        let recovered: ProofArchiveEntry = serde_json::from_slice(&decompressed).unwrap();
        assert_eq!(recovered.id, "old-proof");
        assert_eq!(recovered.result_id, "0xold");
        assert_eq!(recovered.model_hash, "0xmodel");
    }

    #[tokio::test]
    async fn test_dry_run_does_not_move_anything() {
        let tmp = tempdir();
        let archive = ProofArchive::new(&tmp).await.unwrap();
        let cold = tempdir();

        // Create an old date directory with a proof
        let old_dir = tmp.join("2018-06-01");
        tokio::fs::create_dir_all(&old_dir).await.unwrap();
        tokio::fs::write(old_dir.join("proof-dryrun.json"), r#"{"id":"dryrun"}"#)
            .await
            .unwrap();

        // Dry-run should report the file but not move it
        let archived = archive.archive_expired(30, &cold, true).await.unwrap();
        assert_eq!(archived.len(), 1);

        // Original file should still exist
        assert!(old_dir.join("proof-dryrun.json").exists());

        // Cold storage should have no date directories
        let cold_dirs = list_date_dirs(&cold).await.unwrap();
        assert!(cold_dirs.is_empty());
    }

    #[tokio::test]
    async fn test_compressed_archive_is_valid_gzip() {
        // Test the compress/decompress roundtrip directly
        let original = br#"{"id":"test","result_id":"0xabc","features":[1.0,2.0,3.0]}"#;
        let compressed = compress_gzip(original).unwrap();

        // Compressed should be different from original (and typically smaller for JSON)
        assert_ne!(compressed, original.as_slice());

        // Gzip magic number check
        assert_eq!(compressed[0], 0x1f);
        assert_eq!(compressed[1], 0x8b);

        // Decompress and verify
        let decompressed = decompress_gzip(&compressed).unwrap();
        assert_eq!(decompressed, original.as_slice());

        // Parse the decompressed JSON to confirm it's valid
        let entry: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
        assert_eq!(entry["id"], "test");
        assert_eq!(entry["result_id"], "0xabc");
    }

    #[tokio::test]
    async fn test_default_ttl_days_env_var() {
        // Without the env var set, default should be 365
        std::env::remove_var("PROOF_TTL_DAYS");
        assert_eq!(default_ttl_days(), 365);
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
