//! SQLite-backed proof storage and indexing.
//!
//! Proof bundles are stored as JSON files on the filesystem (for portability
//! and easy backup), while SQLite indexes metadata for fast search.

use std::path::PathBuf;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use zkml_verifier::ProofBundle;

use crate::receipt::VerificationReceipt;

/// A stored proof record combining metadata index and filesystem location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProof {
    pub id: String,
    pub model_hash: String,
    pub circuit_hash: String,
    pub created_at: String,
    pub verified: Option<bool>,
    pub verified_at: Option<String>,
    pub proof_size_bytes: usize,
    /// Whether this proof has been soft-deleted.
    #[serde(default)]
    pub deleted: bool,
}

/// Metadata submitted alongside a proof bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofMetadata {
    #[serde(default)]
    pub model_hash: String,
    #[serde(default)]
    pub circuit_hash: String,
}

// VerificationReceipt is defined in crate::receipt and re-exported here for DB operations.

/// Database statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbStats {
    pub total_proofs: usize,
    pub active_proofs: usize,
    pub verified_count: usize,
    pub failed_count: usize,
    pub total_storage_bytes: u64,
}

/// SQLite-backed proof index with filesystem storage for bundles.
pub struct ProofDb {
    conn: Connection,
    storage_dir: PathBuf,
}

impl ProofDb {
    /// Open or create a proof database at `db_path` with bundles stored in `storage_dir`.
    ///
    /// Creates the schema if it doesn't exist.
    pub fn new(db_path: &str, storage_dir: &str) -> Result<Self, String> {
        let conn =
            Connection::open(db_path).map_err(|e| format!("failed to open database: {e}"))?;

        // Enable WAL mode for better concurrent read performance.
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("failed to set WAL mode: {e}"))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS proofs (
                id              TEXT PRIMARY KEY,
                model_hash      TEXT NOT NULL DEFAULT '',
                circuit_hash    TEXT NOT NULL DEFAULT '',
                created_at      TEXT NOT NULL,
                verified        INTEGER,
                verified_at     TEXT,
                proof_size_bytes INTEGER NOT NULL DEFAULT 0,
                deleted         INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_proofs_model_hash ON proofs(model_hash);
            CREATE INDEX IF NOT EXISTS idx_proofs_created_at ON proofs(created_at);
            CREATE INDEX IF NOT EXISTS idx_proofs_circuit_hash ON proofs(circuit_hash);

            CREATE TABLE IF NOT EXISTS verification_receipts (
                id              TEXT PRIMARY KEY,
                proof_id        TEXT NOT NULL,
                verified        INTEGER NOT NULL,
                verified_at     TEXT NOT NULL,
                circuit_hash    TEXT NOT NULL DEFAULT '',
                model_hash      TEXT NOT NULL DEFAULT '',
                verifier_version TEXT NOT NULL DEFAULT '',
                verifier_host   TEXT NOT NULL DEFAULT '',
                signature       TEXT NOT NULL DEFAULT '',
                error           TEXT,
                FOREIGN KEY (proof_id) REFERENCES proofs(id)
            );",
        )
        .map_err(|e| format!("failed to create schema: {e}"))?;

        let storage_dir = PathBuf::from(storage_dir);
        std::fs::create_dir_all(&storage_dir)
            .map_err(|e| format!("failed to create storage dir: {e}"))?;

        Ok(Self { conn, storage_dir })
    }

    /// Store a proof bundle and its metadata. Returns the proof ID.
    pub fn store(
        &self,
        id: &str,
        bundle: &ProofBundle,
        metadata: &ProofMetadata,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();

        // Write bundle to filesystem.
        let bundle_path = self.bundle_path(id);
        let json = serde_json::to_string(bundle)
            .map_err(|e| format!("failed to serialize bundle: {e}"))?;
        let size = json.len();

        // Atomic write: temp file then rename.
        let tmp_path = self.storage_dir.join(format!(".{id}.tmp"));
        std::fs::write(&tmp_path, &json)
            .map_err(|e| format!("failed to write bundle file: {e}"))?;
        std::fs::rename(&tmp_path, &bundle_path)
            .map_err(|e| format!("failed to rename bundle file: {e}"))?;

        // Insert metadata into SQLite.
        self.conn
            .execute(
                "INSERT INTO proofs (id, model_hash, circuit_hash, created_at, proof_size_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, metadata.model_hash, metadata.circuit_hash, now, size],
            )
            .map_err(|e| format!("failed to insert proof record: {e}"))?;

        Ok(())
    }

    /// Get a stored proof record by ID (excludes soft-deleted).
    pub fn get(&self, id: &str) -> Result<Option<StoredProof>, String> {
        self.conn
            .query_row(
                "SELECT id, model_hash, circuit_hash, created_at, verified, verified_at,
                        proof_size_bytes, deleted
                 FROM proofs WHERE id = ?1 AND deleted = 0",
                params![id],
                |row| {
                    Ok(StoredProof {
                        id: row.get(0)?,
                        model_hash: row.get(1)?,
                        circuit_hash: row.get(2)?,
                        created_at: row.get(3)?,
                        verified: row.get(4)?,
                        verified_at: row.get(5)?,
                        proof_size_bytes: row.get::<_, i64>(6)? as usize,
                        deleted: row.get::<_, i64>(7)? != 0,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("failed to query proof: {e}"))
    }

    /// Search proofs by model hash and/or date range.
    pub fn search(
        &self,
        model_hash: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredProof>, String> {
        let mut sql = String::from(
            "SELECT id, model_hash, circuit_hash, created_at, verified, verified_at,
                    proof_size_bytes, deleted
             FROM proofs WHERE deleted = 0",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(mh) = model_hash {
            sql.push_str(&format!(" AND model_hash = ?{}", param_values.len() + 1));
            param_values.push(Box::new(mh.to_string()));
        }
        if let Some(f) = from {
            sql.push_str(&format!(" AND created_at >= ?{}", param_values.len() + 1));
            param_values.push(Box::new(f.to_string()));
        }
        if let Some(t) = to {
            sql.push_str(&format!(" AND created_at <= ?{}", param_values.len() + 1));
            param_values.push(Box::new(t.to_string()));
        }

        sql.push_str(&format!(
            " ORDER BY created_at DESC LIMIT ?{}",
            param_values.len() + 1
        ));
        param_values.push(Box::new(limit as i64));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("failed to prepare search query: {e}"))?;

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(StoredProof {
                    id: row.get(0)?,
                    model_hash: row.get(1)?,
                    circuit_hash: row.get(2)?,
                    created_at: row.get(3)?,
                    verified: row.get(4)?,
                    verified_at: row.get(5)?,
                    proof_size_bytes: row.get::<_, i64>(6)? as usize,
                    deleted: row.get::<_, i64>(7)? != 0,
                })
            })
            .map_err(|e| format!("failed to execute search query: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("failed to read row: {e}"))?);
        }
        Ok(results)
    }

    /// Mark a proof as verified (or failed).
    pub fn mark_verified(&self, id: &str, result: bool) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        let affected = self
            .conn
            .execute(
                "UPDATE proofs SET verified = ?1, verified_at = ?2 WHERE id = ?3 AND deleted = 0",
                params![result, now, id],
            )
            .map_err(|e| format!("failed to update verification status: {e}"))?;

        if affected == 0 {
            return Err(format!("proof '{id}' not found"));
        }
        Ok(())
    }

    /// Store a verification receipt. The receipt's `receipt_id` is used as the DB primary key.
    pub fn store_receipt(&self, receipt: &VerificationReceipt) -> Result<String, String> {
        self.conn
            .execute(
                "INSERT INTO verification_receipts (id, proof_id, verified, verified_at, circuit_hash, model_hash, verifier_version, verifier_host, signature, error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    receipt.receipt_id,
                    receipt.proof_id,
                    receipt.verified,
                    receipt.verified_at,
                    receipt.circuit_hash,
                    receipt.model_hash,
                    receipt.verifier_version,
                    receipt.verifier_host,
                    receipt.signature,
                    receipt.error,
                ],
            )
            .map_err(|e| format!("failed to store receipt: {e}"))?;
        Ok(receipt.receipt_id.clone())
    }

    /// Get the latest verification receipt for a proof.
    pub fn get_receipt(&self, proof_id: &str) -> Result<Option<VerificationReceipt>, String> {
        self.conn
            .query_row(
                "SELECT id, proof_id, verified, verified_at, circuit_hash, model_hash, verifier_version, verifier_host, signature, error
                 FROM verification_receipts WHERE proof_id = ?1
                 ORDER BY verified_at DESC LIMIT 1",
                params![proof_id],
                |row| {
                    Ok(VerificationReceipt {
                        receipt_id: row.get(0)?,
                        proof_id: row.get(1)?,
                        verified: row.get::<_, i64>(2)? != 0,
                        verified_at: row.get(3)?,
                        circuit_hash: row.get(4)?,
                        model_hash: row.get(5)?,
                        verifier_version: row.get(6)?,
                        verifier_host: row.get(7)?,
                        signature: row.get(8)?,
                        error: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("failed to query receipt: {e}"))
    }

    /// Soft-delete a proof (mark inactive, don't remove from disk).
    pub fn soft_delete(&self, id: &str) -> Result<(), String> {
        let affected = self
            .conn
            .execute(
                "UPDATE proofs SET deleted = 1 WHERE id = ?1 AND deleted = 0",
                params![id],
            )
            .map_err(|e| format!("failed to soft-delete proof: {e}"))?;

        if affected == 0 {
            return Err(format!("proof '{id}' not found or already deleted"));
        }
        Ok(())
    }

    /// Get aggregate statistics.
    pub fn stats(&self) -> Result<DbStats, String> {
        let total_proofs: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM proofs", [], |row| row.get(0))
            .map_err(|e| format!("failed to count proofs: {e}"))?;

        let active_proofs: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM proofs WHERE deleted = 0",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to count active proofs: {e}"))?;

        let verified_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM proofs WHERE verified = 1 AND deleted = 0",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to count verified proofs: {e}"))?;

        let failed_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM proofs WHERE verified = 0 AND verified IS NOT NULL AND deleted = 0",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to count failed proofs: {e}"))?;

        let total_storage_bytes: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(proof_size_bytes), 0) FROM proofs WHERE deleted = 0",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to sum storage: {e}"))?;

        Ok(DbStats {
            total_proofs: total_proofs as usize,
            active_proofs: active_proofs as usize,
            verified_count: verified_count as usize,
            failed_count: failed_count as usize,
            total_storage_bytes: total_storage_bytes as u64,
        })
    }

    /// Load a proof bundle from the filesystem.
    pub fn load_bundle(&self, id: &str) -> Result<ProofBundle, String> {
        let path = self.bundle_path(id);
        let data =
            std::fs::read_to_string(&path).map_err(|e| format!("failed to read bundle: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("failed to parse bundle: {e}"))
    }

    /// Check if the database is healthy (can execute a simple query).
    pub fn is_healthy(&self) -> bool {
        self.conn
            .query_row("SELECT 1", [], |_| Ok(()))
            .is_ok()
    }

    /// Return the file path for a bundle with the given ID.
    fn bundle_path(&self, id: &str) -> PathBuf {
        self.storage_dir.join(format!("{id}.json"))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_bundle() -> ProofBundle {
        let proof_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let gens_data: Vec<u8> = (0..128).map(|i| ((i * 7) % 256) as u8).collect();

        ProofBundle {
            proof_hex: format!("0x{}", hex::encode(&proof_data)),
            public_inputs_hex: String::new(),
            gens_hex: format!("0x{}", hex::encode(&gens_data)),
            dag_circuit_description: serde_json::json!({
                "num_compute_layers": 4,
                "layer_types": [0, 1, 0, 1],
            }),
            model_hash: Some("0xabcdef1234567890".to_string()),
            timestamp: Some(1700000000),
            prover_version: Some("0.1.0-test".to_string()),
            circuit_hash: Some("0xdeadbeef".to_string()),
        }
    }

    fn make_test_db() -> (ProofDb, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let storage_dir = tmp.path().join("bundles");
        let db = ProofDb::new(
            db_path.to_str().unwrap(),
            storage_dir.to_str().unwrap(),
        )
        .unwrap();
        (db, tmp)
    }

    #[test]
    fn test_store_and_retrieve() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();
        let meta = ProofMetadata {
            model_hash: "0xmodel123".to_string(),
            circuit_hash: "0xcircuit456".to_string(),
        };

        db.store("proof-1", &bundle, &meta).unwrap();

        let stored = db.get("proof-1").unwrap().expect("proof should exist");
        assert_eq!(stored.id, "proof-1");
        assert_eq!(stored.model_hash, "0xmodel123");
        assert_eq!(stored.circuit_hash, "0xcircuit456");
        assert!(stored.verified.is_none());
        assert!(stored.proof_size_bytes > 0);
        assert!(!stored.deleted);
    }

    #[test]
    fn test_get_nonexistent() {
        let (db, _tmp) = make_test_db();
        let result = db.get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_search_by_model_hash() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();

        // Store proofs with different model hashes.
        db.store(
            "p1",
            &bundle,
            &ProofMetadata {
                model_hash: "model-a".into(),
                circuit_hash: "c1".into(),
            },
        )
        .unwrap();
        db.store(
            "p2",
            &bundle,
            &ProofMetadata {
                model_hash: "model-b".into(),
                circuit_hash: "c2".into(),
            },
        )
        .unwrap();
        db.store(
            "p3",
            &bundle,
            &ProofMetadata {
                model_hash: "model-a".into(),
                circuit_hash: "c3".into(),
            },
        )
        .unwrap();

        let results = db
            .search(Some("model-a"), None, None, 50)
            .unwrap();
        assert_eq!(results.len(), 2);
        for r in &results {
            assert_eq!(r.model_hash, "model-a");
        }
    }

    #[test]
    fn test_search_by_date_range() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();
        let meta = ProofMetadata {
            model_hash: "m".into(),
            circuit_hash: "c".into(),
        };

        // Store 3 proofs (all have similar timestamps from creation).
        db.store("d1", &bundle, &meta).unwrap();
        db.store("d2", &bundle, &meta).unwrap();
        db.store("d3", &bundle, &meta).unwrap();

        // Search with a wide date range that includes all.
        let results = db
            .search(None, Some("2020-01-01"), Some("2030-12-31"), 50)
            .unwrap();
        assert_eq!(results.len(), 3);

        // Search with a narrow range that excludes all (far future).
        let results = db
            .search(None, Some("2099-01-01"), None, 50)
            .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_limit() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();
        let meta = ProofMetadata {
            model_hash: "m".into(),
            circuit_hash: "c".into(),
        };

        for i in 0..10 {
            db.store(&format!("limit-{i}"), &bundle, &meta).unwrap();
        }

        let results = db.search(None, None, None, 3).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_mark_verified() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();
        let meta = ProofMetadata {
            model_hash: "m".into(),
            circuit_hash: "c".into(),
        };

        db.store("v1", &bundle, &meta).unwrap();

        // Initially unverified.
        let stored = db.get("v1").unwrap().unwrap();
        assert!(stored.verified.is_none());

        // Mark as verified.
        db.mark_verified("v1", true).unwrap();

        let stored = db.get("v1").unwrap().unwrap();
        assert_eq!(stored.verified, Some(true));
        assert!(stored.verified_at.is_some());
    }

    #[test]
    fn test_mark_verified_nonexistent() {
        let (db, _tmp) = make_test_db();
        let result = db.mark_verified("nonexistent", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_soft_delete() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();
        let meta = ProofMetadata {
            model_hash: "m".into(),
            circuit_hash: "c".into(),
        };

        db.store("del-1", &bundle, &meta).unwrap();

        // Proof exists.
        assert!(db.get("del-1").unwrap().is_some());

        // Soft-delete it.
        db.soft_delete("del-1").unwrap();

        // Proof no longer visible.
        assert!(db.get("del-1").unwrap().is_none());

        // Bundle file still exists on disk.
        assert!(db.bundle_path("del-1").exists());
    }

    #[test]
    fn test_soft_delete_nonexistent() {
        let (db, _tmp) = make_test_db();
        let result = db.soft_delete("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_stats() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();
        let meta = ProofMetadata {
            model_hash: "m".into(),
            circuit_hash: "c".into(),
        };

        db.store("s1", &bundle, &meta).unwrap();
        db.store("s2", &bundle, &meta).unwrap();
        db.store("s3", &bundle, &meta).unwrap();

        db.mark_verified("s1", true).unwrap();
        db.mark_verified("s2", false).unwrap();
        db.soft_delete("s3").unwrap();

        let stats = db.stats().unwrap();
        assert_eq!(stats.total_proofs, 3);
        assert_eq!(stats.active_proofs, 2);
        assert_eq!(stats.verified_count, 1);
        assert_eq!(stats.failed_count, 1);
        assert!(stats.total_storage_bytes > 0);
    }

    #[test]
    fn test_load_bundle() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();
        let meta = ProofMetadata {
            model_hash: "m".into(),
            circuit_hash: "c".into(),
        };

        db.store("load-1", &bundle, &meta).unwrap();

        let loaded = db.load_bundle("load-1").unwrap();
        assert_eq!(loaded.proof_hex, bundle.proof_hex);
        assert_eq!(loaded.gens_hex, bundle.gens_hex);
        assert_eq!(loaded.model_hash, bundle.model_hash);
    }

    #[test]
    fn test_store_and_get_receipt() {
        let (db, _tmp) = make_test_db();
        let bundle = make_test_bundle();
        let meta = ProofMetadata {
            model_hash: "m".into(),
            circuit_hash: "c".into(),
        };

        db.store("r1", &bundle, &meta).unwrap();

        let receipt = VerificationReceipt::new("r1", "0xcircuit", "m", true, None);

        let receipt_id = db.store_receipt(&receipt).unwrap();
        assert!(!receipt_id.is_empty());
        assert_eq!(receipt_id, receipt.receipt_id);

        let loaded = db.get_receipt("r1").unwrap().expect("receipt should exist");
        assert_eq!(loaded.proof_id, "r1");
        assert!(loaded.verified);
        assert_eq!(loaded.circuit_hash, "0xcircuit");
        assert_eq!(loaded.model_hash, "m");
        assert!(!loaded.verifier_version.is_empty());
        assert!(loaded.error.is_none());
    }

    #[test]
    fn test_health_check() {
        let (db, _tmp) = make_test_db();
        assert!(db.is_healthy());
    }
}
