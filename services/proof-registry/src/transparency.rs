//! Append-only Merkle tree transparency log for proof bundles.
//!
//! Similar to Certificate Transparency (RFC 6962), this module maintains an
//! append-only log of SHA-256 proof hashes. Anyone can verify that a specific
//! proof was submitted at a given index by checking a Merkle inclusion proof
//! against the published root.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// An entry in the transparency log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Leaf index (0-based).
    pub index: u64,
    /// SHA-256 hash of the proof bundle JSON.
    pub proof_hash: String,
    /// Proof ID in the registry (for cross-reference).
    pub proof_id: String,
    /// RFC 3339 timestamp of when the entry was appended.
    pub appended_at: String,
}

/// Merkle inclusion proof for a specific leaf.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InclusionProof {
    /// The leaf index this proof is for.
    pub index: u64,
    /// The leaf hash at that index.
    pub leaf_hash: String,
    /// Sibling hashes from leaf to root (hex-encoded).
    pub proof: Vec<String>,
    /// The Merkle root at the time of proof generation.
    pub root: String,
    /// Tree size when this proof was generated.
    pub tree_size: u64,
}

/// Request body for the verify endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyRequest {
    pub index: u64,
    pub leaf_hash: String,
    pub proof: Vec<String>,
    pub root: String,
    pub tree_size: u64,
}

/// Response from the verify endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResponse {
    pub valid: bool,
}

/// Response from the root endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootResponse {
    pub root: String,
    pub tree_size: u64,
}

/// Response from the entries endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntriesResponse {
    pub entries: Vec<LogEntry>,
    pub total: u64,
}

/// Append-only Merkle tree transparency log backed by SQLite.
///
/// Leaves are stored in insertion order. The Merkle tree is computed on-the-fly
/// from the leaf list, which is efficient for moderate log sizes (millions of
/// entries). For very large logs, a cached internal-node approach would be
/// better, but this suffices for a proof registry.
pub struct TransparencyLog {
    conn: Connection,
}

impl TransparencyLog {
    /// Open or create a transparency log database at `db_path`.
    ///
    /// Creates the schema if it does not exist.
    pub fn new(db_path: &str) -> Result<Self, String> {
        let conn =
            Connection::open(db_path).map_err(|e| format!("failed to open log database: {e}"))?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("failed to set WAL mode: {e}"))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS transparency_log (
                idx         INTEGER PRIMARY KEY,
                proof_hash  BLOB NOT NULL,
                proof_id    TEXT NOT NULL,
                appended_at TEXT NOT NULL
            );",
        )
        .map_err(|e| format!("failed to create transparency schema: {e}"))?;

        Ok(Self { conn })
    }

    /// Append a proof hash to the log. Returns the leaf index.
    pub fn append(
        &mut self,
        proof_hash: [u8; 32],
        proof_id: &str,
    ) -> Result<u64, String> {
        let now = chrono::Utc::now().to_rfc3339();
        let next_idx = self.size()?;

        self.conn
            .execute(
                "INSERT INTO transparency_log (idx, proof_hash, proof_id, appended_at) VALUES (?1, ?2, ?3, ?4)",
                params![next_idx as i64, proof_hash.as_slice(), proof_id, now],
            )
            .map_err(|e| format!("failed to append to transparency log: {e}"))?;

        Ok(next_idx)
    }

    /// Get the current Merkle root. Returns all-zeros for an empty tree.
    pub fn root(&self) -> Result<[u8; 32], String> {
        let leaves = self.all_leaves()?;
        Ok(compute_root(&leaves))
    }

    /// Get the number of entries in the log.
    pub fn size(&self) -> Result<u64, String> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM transparency_log", [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("failed to count log entries: {e}"))?;
        Ok(count as u64)
    }

    /// Generate a Merkle inclusion proof for the leaf at `index`.
    pub fn inclusion_proof(&self, index: u64) -> Result<Vec<[u8; 32]>, String> {
        let leaves = self.all_leaves()?;
        let n = leaves.len();

        if index as usize >= n {
            return Err(format!(
                "index {index} out of range (tree size: {n})"
            ));
        }

        Ok(merkle_proof(&leaves, index as usize))
    }

    /// Get a leaf hash by index.
    pub fn get_leaf(&self, index: u64) -> Result<Option<[u8; 32]>, String> {
        let result: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT proof_hash FROM transparency_log WHERE idx = ?1",
                params![index as i64],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("failed to query leaf: {e}"))?;

        match result {
            Some(bytes) => {
                let hash: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| "stored hash is not 32 bytes".to_string())?;
                Ok(Some(hash))
            }
            None => Ok(None),
        }
    }

    /// Get a log entry by index.
    pub fn get_entry(&self, index: u64) -> Result<Option<LogEntry>, String> {
        self.conn
            .query_row(
                "SELECT idx, proof_hash, proof_id, appended_at FROM transparency_log WHERE idx = ?1",
                params![index as i64],
                |row| {
                    let hash_bytes: Vec<u8> = row.get(1)?;
                    Ok(LogEntry {
                        index: row.get::<_, i64>(0)? as u64,
                        proof_hash: hex::encode(hash_bytes),
                        proof_id: row.get(2)?,
                        appended_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("failed to query entry: {e}"))
    }

    /// List entries starting from `from` up to `count` entries.
    pub fn list_entries(&self, from: u64, count: u64) -> Result<Vec<LogEntry>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT idx, proof_hash, proof_id, appended_at
                 FROM transparency_log
                 WHERE idx >= ?1
                 ORDER BY idx ASC
                 LIMIT ?2",
            )
            .map_err(|e| format!("failed to prepare list query: {e}"))?;

        let rows = stmt
            .query_map(params![from as i64, count as i64], |row| {
                let hash_bytes: Vec<u8> = row.get(1)?;
                Ok(LogEntry {
                    index: row.get::<_, i64>(0)? as u64,
                    proof_hash: hex::encode(hash_bytes),
                    proof_id: row.get(2)?,
                    appended_at: row.get(3)?,
                })
            })
            .map_err(|e| format!("failed to query entries: {e}"))?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| format!("failed to read entry: {e}"))?);
        }
        Ok(entries)
    }

    /// Load all leaf hashes in order (for Merkle computation).
    fn all_leaves(&self) -> Result<Vec<[u8; 32]>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT proof_hash FROM transparency_log ORDER BY idx ASC")
            .map_err(|e| format!("failed to prepare leaves query: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                let bytes: Vec<u8> = row.get(0)?;
                Ok(bytes)
            })
            .map_err(|e| format!("failed to query leaves: {e}"))?;

        let mut leaves = Vec::new();
        for row in rows {
            let bytes = row.map_err(|e| format!("failed to read leaf: {e}"))?;
            let hash: [u8; 32] = bytes
                .try_into()
                .map_err(|_| "stored hash is not 32 bytes".to_string())?;
            leaves.push(hash);
        }
        Ok(leaves)
    }

    /// Check if the log database is healthy.
    pub fn is_healthy(&self) -> bool {
        self.conn
            .query_row("SELECT 1", [], |_| Ok(()))
            .is_ok()
    }
}

/// Compute the SHA-256 hash of a proof bundle's JSON serialization.
pub fn hash_proof_bundle(bundle_json: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bundle_json);
    hasher.finalize().into()
}

/// Verify a Merkle inclusion proof.
///
/// Given a `root`, a `leaf` hash, the leaf's `index`, and a `proof` (sibling
/// path), returns `true` if the proof is valid.
pub fn verify_inclusion(
    root: &[u8; 32],
    leaf: &[u8; 32],
    index: u64,
    proof: &[[u8; 32]],
    tree_size: u64,
) -> bool {
    if tree_size == 0 {
        return false;
    }
    if index >= tree_size {
        return false;
    }

    // Recompute the expected proof length.
    let expected_depth = tree_depth(tree_size as usize);
    if proof.len() != expected_depth {
        return false;
    }

    let mut current = *leaf;
    let mut idx = index;

    for sibling in proof {
        if idx & 1 == 0 {
            // Current node is a left child.
            current = hash_pair(&current, sibling);
        } else {
            // Current node is a right child.
            current = hash_pair(sibling, &current);
        }
        idx >>= 1;
    }

    current == *root
}

// -- Internal Merkle helpers --

/// Compute the Merkle root of a list of leaf hashes.
///
/// Uses a standard binary Merkle tree. If the number of leaves is not a power
/// of two, the tree is padded with zero-hashes (empty leaves) on the right.
fn compute_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }

    let depth = tree_depth(leaves.len());
    let padded_size = 1usize << depth;

    // Start with leaves, padded to a power of two.
    let mut level: Vec<[u8; 32]> = Vec::with_capacity(padded_size);
    level.extend_from_slice(leaves);
    level.resize(padded_size, [0u8; 32]);

    // Iteratively hash pairs up to the root.
    while level.len() > 1 {
        let mut next_level = Vec::with_capacity(level.len() / 2);
        for pair in level.chunks(2) {
            next_level.push(hash_pair(&pair[0], &pair[1]));
        }
        level = next_level;
    }

    level[0]
}

/// Generate a Merkle proof (list of sibling hashes from leaf to root).
fn merkle_proof(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
    let depth = tree_depth(leaves.len());
    let padded_size = 1usize << depth;

    let mut level: Vec<[u8; 32]> = Vec::with_capacity(padded_size);
    level.extend_from_slice(leaves);
    level.resize(padded_size, [0u8; 32]);

    let mut proof = Vec::with_capacity(depth);
    let mut idx = index;

    for _ in 0..depth {
        // Sibling is the node we are NOT on.
        let sibling_idx = idx ^ 1;
        proof.push(level[sibling_idx]);

        // Move up one level.
        let mut next_level = Vec::with_capacity(level.len() / 2);
        for pair in level.chunks(2) {
            next_level.push(hash_pair(&pair[0], &pair[1]));
        }
        level = next_level;
        idx >>= 1;
    }

    proof
}

/// Hash two sibling nodes together: SHA-256(left || right).
fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Compute the depth of a binary tree that can hold `n` leaves.
/// Returns 0 for n <= 1, ceil(log2(n)) otherwise.
fn tree_depth(n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    // ceil(log2(n))
    let mut depth = 0;
    let mut size = 1;
    while size < n {
        size <<= 1;
        depth += 1;
    }
    depth
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_log() -> (TransparencyLog, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("transparency.db");
        let log = TransparencyLog::new(db_path.to_str().unwrap()).unwrap();
        (log, tmp)
    }

    fn make_hash(seed: u8) -> [u8; 32] {
        let mut h = [0u8; 32];
        h[0] = seed;
        h[31] = seed.wrapping_mul(7);
        h
    }

    #[test]
    fn test_empty_log() {
        let (log, _tmp) = make_test_log();
        assert_eq!(log.size().unwrap(), 0);
        assert_eq!(log.root().unwrap(), [0u8; 32]);
    }

    #[test]
    fn test_append_and_retrieve() {
        let (mut log, _tmp) = make_test_log();

        let hash = make_hash(1);
        let idx = log.append(hash, "proof-1").unwrap();
        assert_eq!(idx, 0);
        assert_eq!(log.size().unwrap(), 1);

        let leaf = log.get_leaf(0).unwrap().unwrap();
        assert_eq!(leaf, hash);

        let entry = log.get_entry(0).unwrap().unwrap();
        assert_eq!(entry.proof_id, "proof-1");
        assert_eq!(entry.index, 0);
    }

    #[test]
    fn test_root_changes_after_append() {
        let (mut log, _tmp) = make_test_log();

        let hash1 = make_hash(1);
        log.append(hash1, "p1").unwrap();
        let root1 = log.root().unwrap();

        let hash2 = make_hash(2);
        log.append(hash2, "p2").unwrap();
        let root2 = log.root().unwrap();

        // Root must change after appending a new leaf.
        assert_ne!(root1, root2);
        // Neither root should be zero (non-empty tree).
        assert_ne!(root1, [0u8; 32]);
        assert_ne!(root2, [0u8; 32]);
    }

    #[test]
    fn test_single_leaf_root() {
        let (mut log, _tmp) = make_test_log();
        let hash = make_hash(42);
        log.append(hash, "p1").unwrap();

        // A single-leaf tree's root is the leaf itself.
        let root = log.root().unwrap();
        assert_eq!(root, hash);
    }

    #[test]
    fn test_two_leaf_root() {
        let (mut log, _tmp) = make_test_log();
        let h1 = make_hash(1);
        let h2 = make_hash(2);
        log.append(h1, "p1").unwrap();
        log.append(h2, "p2").unwrap();

        let expected = hash_pair(&h1, &h2);
        let root = log.root().unwrap();
        assert_eq!(root, expected);
    }

    #[test]
    fn test_inclusion_proof_valid() {
        let (mut log, _tmp) = make_test_log();

        // Append several leaves.
        let hashes: Vec<[u8; 32]> = (0..5).map(|i| make_hash(i)).collect();
        for (i, h) in hashes.iter().enumerate() {
            log.append(*h, &format!("p{i}")).unwrap();
        }

        let root = log.root().unwrap();
        let tree_size = log.size().unwrap();

        // Verify inclusion for each leaf.
        for (i, h) in hashes.iter().enumerate() {
            let proof = log.inclusion_proof(i as u64).unwrap();
            assert!(
                verify_inclusion(&root, h, i as u64, &proof, tree_size),
                "inclusion proof failed for leaf {i}"
            );
        }
    }

    #[test]
    fn test_inclusion_proof_invalid_leaf() {
        let (mut log, _tmp) = make_test_log();

        let h1 = make_hash(1);
        let h2 = make_hash(2);
        log.append(h1, "p1").unwrap();
        log.append(h2, "p2").unwrap();

        let root = log.root().unwrap();
        let tree_size = log.size().unwrap();
        let proof = log.inclusion_proof(0).unwrap();

        // Using the wrong leaf hash should fail.
        let wrong_hash = make_hash(99);
        assert!(!verify_inclusion(
            &root, &wrong_hash, 0, &proof, tree_size
        ));
    }

    #[test]
    fn test_inclusion_proof_invalid_index() {
        let (mut log, _tmp) = make_test_log();

        let h1 = make_hash(1);
        let h2 = make_hash(2);
        log.append(h1, "p1").unwrap();
        log.append(h2, "p2").unwrap();

        let root = log.root().unwrap();
        let tree_size = log.size().unwrap();
        let proof = log.inclusion_proof(0).unwrap();

        // Using the proof for index 0 but claiming index 1 should fail.
        assert!(!verify_inclusion(&root, &h1, 1, &proof, tree_size));
    }

    #[test]
    fn test_inclusion_proof_wrong_root() {
        let (mut log, _tmp) = make_test_log();

        let h1 = make_hash(1);
        log.append(h1, "p1").unwrap();

        let tree_size = log.size().unwrap();
        let proof = log.inclusion_proof(0).unwrap();

        let wrong_root = make_hash(255);
        assert!(!verify_inclusion(
            &wrong_root, &h1, 0, &proof, tree_size
        ));
    }

    #[test]
    fn test_out_of_range_index() {
        let (mut log, _tmp) = make_test_log();
        log.append(make_hash(1), "p1").unwrap();

        let result = log.inclusion_proof(5);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_entries() {
        let (mut log, _tmp) = make_test_log();

        for i in 0..10u8 {
            log.append(make_hash(i), &format!("p{i}")).unwrap();
        }

        // Get entries 3..8.
        let entries = log.list_entries(3, 5).unwrap();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].index, 3);
        assert_eq!(entries[4].index, 7);

        // Get all entries.
        let all = log.list_entries(0, 100).unwrap();
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn test_verify_empty_tree() {
        assert!(!verify_inclusion(
            &[0u8; 32],
            &[0u8; 32],
            0,
            &[],
            0
        ));
    }

    #[test]
    fn test_verify_index_out_of_range() {
        let h = make_hash(1);
        // tree_size=1 but index=1 => out of range.
        assert!(!verify_inclusion(&h, &h, 1, &[], 1));
    }

    #[test]
    fn test_verify_wrong_proof_length() {
        let h = make_hash(1);
        // tree_size=1, depth=0, but providing a non-empty proof.
        assert!(!verify_inclusion(&h, &h, 0, &[[0u8; 32]], 1));
    }

    #[test]
    fn test_large_tree_inclusion() {
        let (mut log, _tmp) = make_test_log();

        let n = 100;
        let hashes: Vec<[u8; 32]> = (0..n).map(|i| make_hash(i as u8)).collect();
        for (i, h) in hashes.iter().enumerate() {
            log.append(*h, &format!("p{i}")).unwrap();
        }

        let root = log.root().unwrap();
        let tree_size = log.size().unwrap();
        assert_eq!(tree_size, n as u64);

        // Spot-check a few indices.
        for &idx in &[0, 1, 49, 50, 99] {
            let proof = log.inclusion_proof(idx as u64).unwrap();
            assert!(
                verify_inclusion(&root, &hashes[idx], idx as u64, &proof, tree_size),
                "inclusion proof failed for leaf {idx} in {n}-leaf tree"
            );
        }
    }

    #[test]
    fn test_hash_proof_bundle() {
        let data = b"{\"proof_hex\":\"0x00\"}";
        let h1 = hash_proof_bundle(data);
        let h2 = hash_proof_bundle(data);
        assert_eq!(h1, h2); // Deterministic.

        let h3 = hash_proof_bundle(b"different data");
        assert_ne!(h1, h3); // Different input => different hash.
    }

    #[test]
    fn test_health_check() {
        let (log, _tmp) = make_test_log();
        assert!(log.is_healthy());
    }
}
