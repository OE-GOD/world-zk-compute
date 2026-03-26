//! Audit log export for compliance reporting.
//!
//! Exports operator audit events in CSV or JSON format for SIEM integration,
//! regulatory compliance, and forensic analysis.
//!
//! Supports two record types:
//! - [`AuditEntry`] — general operator event log entries (submissions, challenges, etc.)
//! - [`AuditRecord`] — proof-archive-derived records for compliance teams
//!
//! The [`scan_archive`] function reads proof bundles from the date-partitioned
//! archive directory and converts them to [`AuditRecord`]s filtered by date range.

use crate::proof_archive::ProofArchiveEntry;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// Event type (e.g., "submission", "challenge", "dispute_resolved").
    pub event_type: String,
    /// On-chain result ID (hex).
    #[serde(default)]
    pub result_id: String,
    /// Transaction hash (hex).
    #[serde(default)]
    pub tx_hash: String,
    /// Block number.
    #[serde(default)]
    pub block_number: u64,
    /// Actor address (submitter, challenger, etc.).
    #[serde(default)]
    pub actor: String,
    /// Model hash (hex).
    #[serde(default)]
    pub model_hash: String,
    /// Human-readable details.
    #[serde(default)]
    pub details: String,
    /// Outcome (success, failure, timeout).
    #[serde(default)]
    pub outcome: String,
    /// Gas used.
    #[serde(default)]
    pub gas_used: u64,
}

/// Export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
    JsonLines,
}

impl ExportFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "csv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            "jsonl" | "jsonlines" | "ndjson" => Some(Self::JsonLines),
            _ => None,
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
            Self::JsonLines => "jsonl",
        }
    }
}

/// Export audit entries to a writer in the specified format.
pub fn export_entries<W: Write>(
    entries: &[AuditEntry],
    format: ExportFormat,
    writer: &mut W,
) -> std::io::Result<()> {
    match format {
        ExportFormat::Csv => export_csv(entries, writer),
        ExportFormat::Json => export_json(entries, writer),
        ExportFormat::JsonLines => export_jsonlines(entries, writer),
    }
}

/// Export to a file, inferring format from extension or explicit format.
pub fn export_to_file(
    entries: &[AuditEntry],
    path: &Path,
    format: Option<ExportFormat>,
) -> std::io::Result<()> {
    let format = format.unwrap_or_else(|| {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(ExportFormat::from_str)
            .unwrap_or(ExportFormat::Json)
    });

    let mut file = std::fs::File::create(path)?;
    export_entries(entries, format, &mut file)
}

// ---------------------------------------------------------------------------
// AuditRecord — proof-archive-derived compliance record
// ---------------------------------------------------------------------------

/// A compliance-oriented audit record derived from [`ProofArchiveEntry`].
///
/// This is the primary export format used by compliance teams to track
/// inference proof records, verification status, and dispute outcomes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    /// ISO-8601 timestamp when the proof was archived.
    pub timestamp: String,
    /// Model hash (hex).
    pub model_hash: String,
    /// Input hash (hex).
    pub input_hash: String,
    /// Inference output / result (hex).
    pub output: String,
    /// Proof / result hash (hex) — uniquely identifies the proof bundle.
    pub proof_hash: String,
    /// Verification status: "finalized", "disputed", or "pending".
    pub verification_status: String,
    /// Gas used for on-chain submission (if known).
    pub gas_used: Option<u64>,
}

impl From<&ProofArchiveEntry> for AuditRecord {
    fn from(entry: &ProofArchiveEntry) -> Self {
        let verification_status = if entry.disputed {
            "disputed".to_string()
        } else if entry.finalized {
            "finalized".to_string()
        } else {
            "pending".to_string()
        };

        Self {
            timestamp: entry.archived_at.clone(),
            model_hash: entry.model_hash.clone(),
            input_hash: entry.input_hash.clone(),
            output: entry.result.clone(),
            proof_hash: entry.result_hash.clone(),
            verification_status,
            gas_used: None,
        }
    }
}

/// Scan the proof archive directory for entries within the given date range.
///
/// Reads all `proof-*.json` files from `archive_dir/YYYY-MM-DD/` directories
/// where the directory date is between `from` and `to` (inclusive, YYYY-MM-DD format).
///
/// Returns `AuditRecord`s sorted by timestamp (ascending).
pub fn scan_archive(
    archive_dir: &Path,
    from: &str,
    to: &str,
) -> std::io::Result<Vec<AuditRecord>> {
    let mut records = Vec::new();

    // Read date directories
    let entries = std::fs::read_dir(archive_dir)?;
    let mut date_dirs: Vec<std::path::PathBuf> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                // Check YYYY-MM-DD pattern and date range
                if name.len() == 10
                    && name.chars().filter(|c| *c == '-').count() == 2
                    && name >= from
                    && name <= to
                {
                    date_dirs.push(path);
                }
            }
        }
    }

    date_dirs.sort();

    for dir in &date_dirs {
        let dir_entries = std::fs::read_dir(dir)?;
        for file_entry in dir_entries {
            let file_entry = file_entry?;
            let file_path = file_entry.path();
            if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("proof-") && name.ends_with(".json") && !name.starts_with('.')
                {
                    match std::fs::read_to_string(&file_path) {
                        Ok(contents) => {
                            match serde_json::from_str::<ProofArchiveEntry>(&contents) {
                                Ok(archive_entry) => {
                                    records.push(AuditRecord::from(&archive_entry));
                                }
                                Err(e) => {
                                    // Log warning but skip malformed files
                                    eprintln!(
                                        "Warning: skipping malformed proof file {}: {}",
                                        file_path.display(),
                                        e
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: could not read {}: {}",
                                file_path.display(),
                                e
                            );
                        }
                    }
                }
            }
        }
    }

    // Sort by timestamp ascending
    records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    Ok(records)
}

/// Export [`AuditRecord`]s as CSV to the given output path.
///
/// Returns the number of records written.
pub fn export_records_csv(records: &[AuditRecord], output: &Path) -> std::io::Result<usize> {
    let mut file = std::fs::File::create(output)?;
    write_records_csv(records, &mut file)?;
    Ok(records.len())
}

/// Export [`AuditRecord`]s as JSON to the given output path.
///
/// Returns the number of records written.
pub fn export_records_json(records: &[AuditRecord], output: &Path) -> std::io::Result<usize> {
    let mut file = std::fs::File::create(output)?;
    write_records_json(records, &mut file)?;
    Ok(records.len())
}

/// Export [`AuditRecord`]s in the specified format to the given output path.
///
/// Returns the number of records written.
pub fn export_records(
    records: &[AuditRecord],
    format: ExportFormat,
    output: &Path,
) -> std::io::Result<usize> {
    let mut file = std::fs::File::create(output)?;
    match format {
        ExportFormat::Csv => write_records_csv(records, &mut file)?,
        ExportFormat::Json => write_records_json(records, &mut file)?,
        ExportFormat::JsonLines => write_records_jsonlines(records, &mut file)?,
    }
    Ok(records.len())
}

/// Write [`AuditRecord`]s as CSV to a writer.
pub fn write_records_csv<W: Write>(records: &[AuditRecord], writer: &mut W) -> std::io::Result<()> {
    writeln!(
        writer,
        "timestamp,model_hash,input_hash,output,proof_hash,verification_status,gas_used"
    )?;
    for r in records {
        writeln!(
            writer,
            "{},{},{},\"{}\",{},{},{}",
            r.timestamp,
            r.model_hash,
            r.input_hash,
            r.output.replace('"', "\"\""),
            r.proof_hash,
            r.verification_status,
            r.gas_used.map_or(String::new(), |g| g.to_string()),
        )?;
    }
    Ok(())
}

/// Write [`AuditRecord`]s as JSON to a writer.
pub fn write_records_json<W: Write>(
    records: &[AuditRecord],
    writer: &mut W,
) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(records)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    writer.write_all(json.as_bytes())
}

/// Write [`AuditRecord`]s as JSON Lines to a writer.
pub fn write_records_jsonlines<W: Write>(
    records: &[AuditRecord],
    writer: &mut W,
) -> std::io::Result<()> {
    for r in records {
        let line = serde_json::to_string(r)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writeln!(writer, "{}", line)?;
    }
    Ok(())
}

fn export_csv<W: Write>(entries: &[AuditEntry], writer: &mut W) -> std::io::Result<()> {
    writeln!(
        writer,
        "timestamp,event_type,result_id,tx_hash,block_number,actor,model_hash,outcome,gas_used,details"
    )?;
    for e in entries {
        writeln!(
            writer,
            "{},{},{},{},{},{},{},{},{},\"{}\"",
            e.timestamp,
            e.event_type,
            e.result_id,
            e.tx_hash,
            e.block_number,
            e.actor,
            e.model_hash,
            e.outcome,
            e.gas_used,
            e.details.replace('"', "\"\""),
        )?;
    }
    Ok(())
}

fn export_json<W: Write>(entries: &[AuditEntry], writer: &mut W) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(entries)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    writer.write_all(json.as_bytes())
}

fn export_jsonlines<W: Write>(entries: &[AuditEntry], writer: &mut W) -> std::io::Result<()> {
    for e in entries {
        let line = serde_json::to_string(e)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writeln!(writer, "{}", line)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<AuditEntry> {
        vec![
            AuditEntry {
                timestamp: "2026-03-26T12:00:00Z".to_string(),
                event_type: "submission".to_string(),
                result_id: "0xabc123".to_string(),
                tx_hash: "0xdef456".to_string(),
                block_number: 12345,
                actor: "0x1234".to_string(),
                model_hash: "0x5678".to_string(),
                details: "Credit scoring inference".to_string(),
                outcome: "success".to_string(),
                gas_used: 250000,
            },
            AuditEntry {
                timestamp: "2026-03-26T12:01:00Z".to_string(),
                event_type: "challenge".to_string(),
                result_id: "0xabc123".to_string(),
                tx_hash: "0x789abc".to_string(),
                block_number: 12346,
                actor: "0xaaaa".to_string(),
                model_hash: String::new(),
                details: "Challenged result".to_string(),
                outcome: "pending".to_string(),
                gas_used: 100000,
            },
        ]
    }

    #[test]
    fn test_export_csv() {
        let entries = sample_entries();
        let mut buf = Vec::new();
        export_entries(&entries, ExportFormat::Csv, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.starts_with("timestamp,event_type,"));
        assert!(output.contains("submission"));
        assert!(output.contains("challenge"));
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 entries
    }

    #[test]
    fn test_export_json() {
        let entries = sample_entries();
        let mut buf = Vec::new();
        export_entries(&entries, ExportFormat::Json, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        let parsed: Vec<AuditEntry> = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].event_type, "submission");
    }

    #[test]
    fn test_export_jsonlines() {
        let entries = sample_entries();
        let mut buf = Vec::new();
        export_entries(&entries, ExportFormat::JsonLines, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: AuditEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.event_type, "submission");
    }

    #[test]
    fn test_export_empty() {
        let mut buf = Vec::new();
        export_entries(&[], ExportFormat::Json, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "[]");
    }

    #[test]
    fn test_format_from_str() {
        assert_eq!(ExportFormat::from_str("csv"), Some(ExportFormat::Csv));
        assert_eq!(ExportFormat::from_str("JSON"), Some(ExportFormat::Json));
        assert_eq!(ExportFormat::from_str("jsonl"), Some(ExportFormat::JsonLines));
        assert_eq!(ExportFormat::from_str("ndjson"), Some(ExportFormat::JsonLines));
        assert_eq!(ExportFormat::from_str("xml"), None);
    }

    #[test]
    fn test_csv_escapes_quotes() {
        let entries = vec![AuditEntry {
            timestamp: "2026-03-26T12:00:00Z".to_string(),
            event_type: "test".to_string(),
            details: "has \"quotes\" inside".to_string(),
            ..Default::default()
        }];
        let mut buf = Vec::new();
        export_entries(&entries, ExportFormat::Csv, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\"\"quotes\"\""));
    }

    // -----------------------------------------------------------------------
    // AuditRecord tests
    // -----------------------------------------------------------------------

    fn make_archive_entry(
        id: &str,
        date: &str,
        model_hash: &str,
        finalized: bool,
        disputed: bool,
    ) -> ProofArchiveEntry {
        ProofArchiveEntry {
            id: id.to_string(),
            archived_at: format!("{}T12:00:00Z", date),
            result_id: format!("0x{}", id),
            model_hash: model_hash.to_string(),
            input_hash: "0xinput".to_string(),
            result_hash: "0xresult".to_string(),
            result: "0x5b302e38355d".to_string(),
            attestation: String::new(),
            features: vec![1.0, 2.0],
            chain_id: 421614,
            enclave_address: String::new(),
            proof_path: None,
            disputed,
            finalized,
        }
    }

    #[test]
    fn test_audit_record_from_finalized() {
        let entry = make_archive_entry("a1", "2026-03-20", "0xmodel", true, false);
        let record = AuditRecord::from(&entry);
        assert_eq!(record.timestamp, "2026-03-20T12:00:00Z");
        assert_eq!(record.model_hash, "0xmodel");
        assert_eq!(record.input_hash, "0xinput");
        assert_eq!(record.proof_hash, "0xresult");
        assert_eq!(record.verification_status, "finalized");
        assert_eq!(record.gas_used, None);
    }

    #[test]
    fn test_audit_record_from_disputed() {
        let entry = make_archive_entry("a2", "2026-03-21", "0xmodel", false, true);
        let record = AuditRecord::from(&entry);
        assert_eq!(record.verification_status, "disputed");
    }

    #[test]
    fn test_audit_record_from_pending() {
        let entry = make_archive_entry("a3", "2026-03-22", "0xmodel", false, false);
        let record = AuditRecord::from(&entry);
        assert_eq!(record.verification_status, "pending");
    }

    #[test]
    fn test_audit_record_disputed_takes_precedence() {
        // If both disputed and finalized are set, "disputed" takes precedence
        let entry = make_archive_entry("a4", "2026-03-22", "0xmodel", true, true);
        let record = AuditRecord::from(&entry);
        assert_eq!(record.verification_status, "disputed");
    }

    #[test]
    fn test_write_records_csv() {
        let records = vec![
            AuditRecord {
                timestamp: "2026-03-20T12:00:00Z".to_string(),
                model_hash: "0xmodel1".to_string(),
                input_hash: "0xinput1".to_string(),
                output: "0xout1".to_string(),
                proof_hash: "0xproof1".to_string(),
                verification_status: "finalized".to_string(),
                gas_used: Some(250000),
            },
            AuditRecord {
                timestamp: "2026-03-21T12:00:00Z".to_string(),
                model_hash: "0xmodel2".to_string(),
                input_hash: "0xinput2".to_string(),
                output: "0xout2".to_string(),
                proof_hash: "0xproof2".to_string(),
                verification_status: "pending".to_string(),
                gas_used: None,
            },
        ];
        let mut buf = Vec::new();
        write_records_csv(&records, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.starts_with("timestamp,model_hash,"));
        assert!(output.contains("0xmodel1"));
        assert!(output.contains("finalized"));
        assert!(output.contains("250000"));
        assert!(output.contains("pending"));
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 records
    }

    #[test]
    fn test_write_records_json() {
        let records = vec![AuditRecord {
            timestamp: "2026-03-20T12:00:00Z".to_string(),
            model_hash: "0xmodel".to_string(),
            input_hash: "0xinput".to_string(),
            output: "0xout".to_string(),
            proof_hash: "0xproof".to_string(),
            verification_status: "finalized".to_string(),
            gas_used: Some(100),
        }];
        let mut buf = Vec::new();
        write_records_json(&records, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        let parsed: Vec<AuditRecord> = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].model_hash, "0xmodel");
        assert_eq!(parsed[0].gas_used, Some(100));
    }

    #[test]
    fn test_write_records_jsonlines() {
        let records = vec![
            AuditRecord {
                timestamp: "2026-03-20T12:00:00Z".to_string(),
                model_hash: "0xa".to_string(),
                input_hash: String::new(),
                output: String::new(),
                proof_hash: String::new(),
                verification_status: "finalized".to_string(),
                gas_used: None,
            },
            AuditRecord {
                timestamp: "2026-03-21T12:00:00Z".to_string(),
                model_hash: "0xb".to_string(),
                input_hash: String::new(),
                output: String::new(),
                proof_hash: String::new(),
                verification_status: "pending".to_string(),
                gas_used: None,
            },
        ];
        let mut buf = Vec::new();
        write_records_jsonlines(&records, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: AuditRecord = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.model_hash, "0xa");
    }

    #[test]
    fn test_write_records_csv_empty() {
        let mut buf = Vec::new();
        write_records_csv(&[], &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1); // header only
    }

    #[test]
    fn test_write_records_json_empty() {
        let mut buf = Vec::new();
        write_records_json(&[], &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "[]");
    }

    /// Helper: create a temporary archive directory with proof files.
    fn setup_test_archive() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("audit-export-test-{}-{}", pid, id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create date directories with proof files
        for (date, entries) in [
            (
                "2026-01-15",
                vec![make_archive_entry("jan1", "2026-01-15", "0xmodelA", true, false)],
            ),
            (
                "2026-02-10",
                vec![
                    make_archive_entry("feb1", "2026-02-10", "0xmodelA", false, true),
                    make_archive_entry("feb2", "2026-02-10", "0xmodelB", true, false),
                ],
            ),
            (
                "2026-03-20",
                vec![make_archive_entry("mar1", "2026-03-20", "0xmodelA", false, false)],
            ),
        ] {
            let date_dir = dir.join(date);
            std::fs::create_dir_all(&date_dir).unwrap();
            for entry in entries {
                let json = serde_json::to_string_pretty(&entry).unwrap();
                std::fs::write(
                    date_dir.join(format!("proof-{}.json", entry.id)),
                    json,
                )
                .unwrap();
            }
        }

        dir
    }

    #[test]
    fn test_scan_archive_full_range() {
        let dir = setup_test_archive();
        let records = scan_archive(&dir, "2026-01-01", "2026-12-31").unwrap();
        assert_eq!(records.len(), 4);
        // Sorted by timestamp ascending
        assert!(records[0].timestamp <= records[1].timestamp);
    }

    #[test]
    fn test_scan_archive_filtered_range() {
        let dir = setup_test_archive();
        let records = scan_archive(&dir, "2026-02-01", "2026-02-28").unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.timestamp.starts_with("2026-02")));
    }

    #[test]
    fn test_scan_archive_single_day() {
        let dir = setup_test_archive();
        let records = scan_archive(&dir, "2026-03-20", "2026-03-20").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].verification_status, "pending");
    }

    #[test]
    fn test_scan_archive_no_match() {
        let dir = setup_test_archive();
        let records = scan_archive(&dir, "2025-01-01", "2025-12-31").unwrap();
        assert_eq!(records.len(), 0);
    }

    #[test]
    fn test_scan_archive_nonexistent_dir() {
        let result = scan_archive(Path::new("/tmp/nonexistent-audit-dir-xyz"), "2026-01-01", "2026-12-31");
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_archive_skips_malformed_files() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("audit-malformed-test-{}", id));
        let _ = std::fs::remove_dir_all(&dir);
        let date_dir = dir.join("2026-03-01");
        std::fs::create_dir_all(&date_dir).unwrap();

        // Write a valid proof file
        let entry = make_archive_entry("good", "2026-03-01", "0xmodel", true, false);
        std::fs::write(
            date_dir.join("proof-good.json"),
            serde_json::to_string(&entry).unwrap(),
        )
        .unwrap();

        // Write a malformed file
        std::fs::write(date_dir.join("proof-bad.json"), "not valid json!!!").unwrap();

        // Write a non-proof file (should be ignored)
        std::fs::write(date_dir.join("other.txt"), "ignored").unwrap();

        let records = scan_archive(&dir, "2026-03-01", "2026-03-31").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model_hash, "0xmodel");
    }

    #[test]
    fn test_export_records_csv_to_file() {
        let dir = setup_test_archive();
        let records = scan_archive(&dir, "2026-01-01", "2026-12-31").unwrap();

        let output_path = dir.join("audit.csv");
        let count = export_records_csv(&records, &output_path).unwrap();
        assert_eq!(count, 4);

        let contents = std::fs::read_to_string(&output_path).unwrap();
        assert!(contents.starts_with("timestamp,model_hash,"));
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 5); // header + 4 records
    }

    #[test]
    fn test_export_records_json_to_file() {
        let dir = setup_test_archive();
        let records = scan_archive(&dir, "2026-01-01", "2026-12-31").unwrap();

        let output_path = dir.join("audit.json");
        let count = export_records_json(&records, &output_path).unwrap();
        assert_eq!(count, 4);

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let parsed: Vec<AuditRecord> = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed.len(), 4);
    }

    #[test]
    fn test_export_records_round_trip() {
        let dir = setup_test_archive();
        let records = scan_archive(&dir, "2026-02-01", "2026-02-28").unwrap();

        // Export to JSON and re-parse
        let mut buf = Vec::new();
        write_records_json(&records, &mut buf).unwrap();
        let parsed: Vec<AuditRecord> = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed.len(), records.len());
        for (orig, rt) in records.iter().zip(parsed.iter()) {
            assert_eq!(orig.timestamp, rt.timestamp);
            assert_eq!(orig.model_hash, rt.model_hash);
            assert_eq!(orig.verification_status, rt.verification_status);
        }
    }
}

impl Default for AuditEntry {
    fn default() -> Self {
        Self {
            timestamp: String::new(),
            event_type: String::new(),
            result_id: String::new(),
            tx_hash: String::new(),
            block_number: 0,
            actor: String::new(),
            model_hash: String::new(),
            details: String::new(),
            outcome: String::new(),
            gas_used: 0,
        }
    }
}
