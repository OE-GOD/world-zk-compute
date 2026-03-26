//! Audit log export for compliance reporting.
//!
//! Exports operator audit events in CSV or JSON format for SIEM integration,
//! regulatory compliance, and forensic analysis.

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
