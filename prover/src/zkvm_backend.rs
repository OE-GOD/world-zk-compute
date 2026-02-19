//! zkVM Backend Abstraction
//!
//! Provides a common trait for different zkVM proving backends (risc0, SP1).
//! Includes auto-detection of VM type from ELF binaries.


use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;

/// Result of executing a guest program without proving.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Public outputs (journal).
    pub journal: Vec<u8>,
    /// Cycle count.
    pub cycles: u64,
    /// Estimated memory usage in bytes.
    pub memory_estimate_bytes: usize,
    /// Number of segments (risc0-specific, 0 for SP1).
    pub segment_count: usize,
    /// Time taken for execution.
    pub execution_time: Duration,
}

/// Result of proving a guest program.
#[derive(Debug, Clone)]
pub struct ProofResult {
    /// Proof seal bytes.
    pub seal: Vec<u8>,
    /// Public outputs (journal).
    pub journal: Vec<u8>,
    /// Cycle count.
    pub cycles: u64,
    /// Time taken for proving.
    pub prove_time: Duration,
}

/// Identifies which zkVM a program targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZkVmType {
    Risc0,
    Sp1,
}

impl std::fmt::Display for ZkVmType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZkVmType::Risc0 => write!(f, "risc0"),
            ZkVmType::Sp1 => write!(f, "SP1"),
        }
    }
}

/// Backend-agnostic proving interface.
///
/// Each zkVM implementation (risc0, SP1) implements this trait,
/// allowing the multi-VM router to dispatch to the correct backend.
#[async_trait]
pub trait ZkVmBackend: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Which VM type this backend supports.
    fn vm_type(&self) -> ZkVmType;

    /// Execute the guest program without generating a proof.
    ///
    /// Used for preflight analysis (cycle estimation, output preview).
    async fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionResult>;

    /// Generate a proof (STARK / compressed).
    async fn prove(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult>;

    /// Generate an on-chain verifiable proof (Groth16 SNARK).
    async fn prove_with_snark(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult>;
}

/// Detect which zkVM a binary targets by examining its contents.
///
/// Heuristic:
/// - SP1 ELFs contain the string "SP1" or specific SP1 markers near the start
/// - Everything else is assumed to be risc0 (the default)
pub fn detect_vm_type(elf: &[u8]) -> ZkVmType {
    // SP1 ELFs are standard RISC-V ELFs compiled with sp1-build.
    // They contain ".sp1" section names or SP1-specific symbols.
    // We search for known SP1 markers in the binary.

    // Check for SP1 section markers (`.sp1` appears in SP1 ELFs)
    if contains_bytes(elf, b".sp1") {
        return ZkVmType::Sp1;
    }

    // Check for SP1 SDK markers in symbol table
    if contains_bytes(elf, b"sp1_zkvm") {
        return ZkVmType::Sp1;
    }

    // Check for SP1 syscall markers
    if contains_bytes(elf, b"sp1_syscall") {
        return ZkVmType::Sp1;
    }

    // Default: risc0
    ZkVmType::Risc0
}

/// Search for a byte pattern in a larger byte slice.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_vm_type_default_risc0() {
        // Random bytes → default to risc0
        let data = vec![0x7f, 0x45, 0x4c, 0x46, 0x01, 0x02, 0x03];
        assert_eq!(detect_vm_type(&data), ZkVmType::Risc0);
    }

    #[test]
    fn test_detect_vm_type_sp1_section() {
        let mut data = vec![0x7f, 0x45, 0x4c, 0x46]; // ELF magic
        data.extend_from_slice(b"some stuff .sp1 more stuff");
        assert_eq!(detect_vm_type(&data), ZkVmType::Sp1);
    }

    #[test]
    fn test_detect_vm_type_sp1_zkvm() {
        let mut data = vec![0u8; 100];
        data.extend_from_slice(b"sp1_zkvm");
        assert_eq!(detect_vm_type(&data), ZkVmType::Sp1);
    }

    #[test]
    fn test_detect_vm_type_empty() {
        assert_eq!(detect_vm_type(&[]), ZkVmType::Risc0);
    }

    #[test]
    fn test_contains_bytes() {
        assert!(contains_bytes(b"hello world", b"world"));
        assert!(!contains_bytes(b"hello world", b"xyz"));
        assert!(!contains_bytes(b"hi", b"hello"));
        assert!(!contains_bytes(b"", b"a"));
    }

    #[test]
    fn test_zkvm_type_display() {
        assert_eq!(format!("{}", ZkVmType::Risc0), "risc0");
        assert_eq!(format!("{}", ZkVmType::Sp1), "SP1");
    }
}
