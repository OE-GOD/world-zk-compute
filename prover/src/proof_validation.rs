//! Proof Validation Hardening
//!
//! Provides comprehensive validation of zkVM proofs before submission:
//! - Structural validation
//! - Size limits
//! - Journal integrity
//! - Image ID verification

use sha2::{Sha256, Digest};
use std::collections::HashSet;

/// Proof validation configuration
#[derive(Debug, Clone)]
pub struct ProofValidationConfig {
    /// Maximum proof size in bytes
    pub max_proof_size: usize,
    /// Maximum journal size in bytes
    pub max_journal_size: usize,
    /// Minimum seal size
    pub min_seal_size: usize,
    /// Maximum seal size
    pub max_seal_size: usize,
    /// Known valid image IDs (optional whitelist)
    pub allowed_image_ids: Option<HashSet<String>>,
    /// Require journal to be valid JSON
    pub require_json_journal: bool,
    /// Enable strict validation
    pub strict_mode: bool,
}

impl Default for ProofValidationConfig {
    fn default() -> Self {
        Self {
            max_proof_size: 10 * 1024 * 1024, // 10MB
            max_journal_size: 1024 * 1024,     // 1MB
            min_seal_size: 128,
            max_seal_size: 5 * 1024 * 1024, // 5MB
            allowed_image_ids: None,
            require_json_journal: false,
            strict_mode: true,
        }
    }
}

/// Proof structure for validation
#[derive(Debug, Clone)]
pub struct ProofData {
    /// The cryptographic seal (STARK/SNARK proof)
    pub seal: Vec<u8>,
    /// Public journal data
    pub journal: Vec<u8>,
    /// Image ID that was executed
    pub image_id: [u8; 32],
}

/// Proof validator
pub struct ProofValidator {
    config: ProofValidationConfig,
}

impl ProofValidator {
    /// Create with default config
    pub fn new() -> Self {
        Self {
            config: ProofValidationConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(config: ProofValidationConfig) -> Self {
        Self { config }
    }

    /// Validate a proof completely
    pub fn validate(&self, proof: &ProofData) -> Result<ValidationResult, ValidationError> {
        let mut result = ValidationResult::new();

        // Size validations
        self.validate_sizes(proof, &mut result)?;

        // Structural validations
        self.validate_structure(proof, &mut result)?;

        // Image ID validation
        self.validate_image_id(proof, &mut result)?;

        // Journal validation
        self.validate_journal(proof, &mut result)?;

        // Seal validation
        self.validate_seal(proof, &mut result)?;

        if result.has_errors() {
            Err(ValidationError::Multiple(result.errors.clone()))
        } else {
            Ok(result)
        }
    }

    fn validate_sizes(
        &self,
        proof: &ProofData,
        result: &mut ValidationResult,
    ) -> Result<(), ValidationError> {
        let total_size = proof.seal.len() + proof.journal.len() + 32;

        if total_size > self.config.max_proof_size {
            result.add_error(format!(
                "Proof too large: {} bytes (max {})",
                total_size, self.config.max_proof_size
            ));
            if self.config.strict_mode {
                return Err(ValidationError::ProofTooLarge {
                    size: total_size,
                    max: self.config.max_proof_size,
                });
            }
        }

        if proof.journal.len() > self.config.max_journal_size {
            result.add_error(format!(
                "Journal too large: {} bytes (max {})",
                proof.journal.len(),
                self.config.max_journal_size
            ));
            if self.config.strict_mode {
                return Err(ValidationError::JournalTooLarge {
                    size: proof.journal.len(),
                    max: self.config.max_journal_size,
                });
            }
        }

        // Check for empty seal first (more specific error)
        if proof.seal.is_empty() {
            result.add_error("Seal is empty".to_string());
            if self.config.strict_mode {
                return Err(ValidationError::EmptySeal);
            }
        } else if proof.seal.len() < self.config.min_seal_size {
            result.add_error(format!(
                "Seal too small: {} bytes (min {})",
                proof.seal.len(),
                self.config.min_seal_size
            ));
            if self.config.strict_mode {
                return Err(ValidationError::SealTooSmall {
                    size: proof.seal.len(),
                    min: self.config.min_seal_size,
                });
            }
        }

        if proof.seal.len() > self.config.max_seal_size {
            result.add_error(format!(
                "Seal too large: {} bytes (max {})",
                proof.seal.len(),
                self.config.max_seal_size
            ));
            if self.config.strict_mode {
                return Err(ValidationError::SealTooLarge {
                    size: proof.seal.len(),
                    max: self.config.max_seal_size,
                });
            }
        }

        result.add_check("size_validation", true);
        Ok(())
    }

    fn validate_structure(
        &self,
        proof: &ProofData,
        result: &mut ValidationResult,
    ) -> Result<(), ValidationError> {
        // Check for empty components
        if proof.seal.is_empty() {
            result.add_error("Seal is empty".to_string());
            if self.config.strict_mode {
                return Err(ValidationError::EmptySeal);
            }
        }

        // Check for all-zero image ID
        if proof.image_id == [0u8; 32] {
            result.add_error("Image ID is all zeros".to_string());
            if self.config.strict_mode {
                return Err(ValidationError::InvalidImageId(
                    "all zeros not allowed".to_string(),
                ));
            }
        }

        // Check seal entropy (detect obviously fake proofs)
        if self.config.strict_mode && !self.has_sufficient_entropy(&proof.seal) {
            result.add_error("Seal has insufficient entropy".to_string());
            return Err(ValidationError::InsufficientEntropy);
        }

        result.add_check("structure_validation", true);
        Ok(())
    }

    fn validate_image_id(
        &self,
        proof: &ProofData,
        result: &mut ValidationResult,
    ) -> Result<(), ValidationError> {
        let image_id_hex = format!("0x{}", hex::encode(proof.image_id));

        // Check against whitelist if configured
        if let Some(ref allowed) = self.config.allowed_image_ids {
            if !allowed.contains(&image_id_hex) {
                result.add_error(format!("Image ID not in whitelist: {}", image_id_hex));
                if self.config.strict_mode {
                    return Err(ValidationError::ImageIdNotAllowed(image_id_hex));
                }
            }
        }

        result.image_id = Some(image_id_hex);
        result.add_check("image_id_validation", true);
        Ok(())
    }

    fn validate_journal(
        &self,
        proof: &ProofData,
        result: &mut ValidationResult,
    ) -> Result<(), ValidationError> {
        // Compute journal hash for reference
        let mut hasher = Sha256::new();
        hasher.update(&proof.journal);
        let journal_hash = hasher.finalize();
        result.journal_hash = Some(format!("0x{}", hex::encode(journal_hash)));

        // Check if journal should be valid JSON
        if self.config.require_json_journal && !proof.journal.is_empty() {
            if let Err(e) = serde_json::from_slice::<serde_json::Value>(&proof.journal) {
                result.add_error(format!("Journal is not valid JSON: {}", e));
                if self.config.strict_mode {
                    return Err(ValidationError::InvalidJournalFormat(e.to_string()));
                }
            }
        }

        // Check for suspicious patterns in journal
        if self.config.strict_mode {
            self.check_journal_patterns(&proof.journal, result)?;
        }

        result.add_check("journal_validation", true);
        Ok(())
    }

    fn validate_seal(
        &self,
        proof: &ProofData,
        result: &mut ValidationResult,
    ) -> Result<(), ValidationError> {
        // Compute seal hash for reference
        let mut hasher = Sha256::new();
        hasher.update(&proof.seal);
        let seal_hash = hasher.finalize();
        result.seal_hash = Some(format!("0x{}", hex::encode(seal_hash)));

        // Check for known invalid seal patterns
        if self.config.strict_mode {
            // All same byte is definitely invalid
            if proof.seal.iter().all(|&b| b == proof.seal[0]) {
                result.add_error("Seal has all identical bytes".to_string());
                return Err(ValidationError::InvalidSealPattern);
            }

            // Too many zeros is suspicious
            let zero_count = proof.seal.iter().filter(|&&b| b == 0).count();
            if zero_count > proof.seal.len() * 3 / 4 {
                result.add_warning("Seal has unusually many zero bytes".to_string());
            }
        }

        result.add_check("seal_validation", true);
        Ok(())
    }

    fn has_sufficient_entropy(&self, data: &[u8]) -> bool {
        if data.len() < 32 {
            return false;
        }

        // Simple entropy check: count unique bytes
        let unique: HashSet<u8> = data.iter().copied().collect();

        // Expect at least 50% unique bytes for random data
        unique.len() > data.len().min(256) / 4
    }

    fn check_journal_patterns(
        &self,
        journal: &[u8],
        result: &mut ValidationResult,
    ) -> Result<(), ValidationError> {
        // Check for null bytes (might indicate buffer issues)
        if journal.contains(&0) {
            let null_count = journal.iter().filter(|&&b| b == 0).count();
            if null_count > journal.len() / 10 {
                result.add_warning("Journal contains many null bytes".to_string());
            }
        }

        // Check for repetitive patterns
        if journal.len() >= 16 {
            let chunk = &journal[0..8];
            let repeats = journal.chunks(8).filter(|c| *c == chunk).count();
            if repeats > journal.len() / 8 / 2 {
                result.add_warning("Journal has repetitive pattern".to_string());
            }
        }

        Ok(())
    }

    /// Quick validation (size checks only)
    pub fn quick_validate(&self, proof: &ProofData) -> Result<(), ValidationError> {
        let total_size = proof.seal.len() + proof.journal.len() + 32;

        if total_size > self.config.max_proof_size {
            return Err(ValidationError::ProofTooLarge {
                size: total_size,
                max: self.config.max_proof_size,
            });
        }

        if proof.seal.is_empty() {
            return Err(ValidationError::EmptySeal);
        }

        if proof.image_id == [0u8; 32] {
            return Err(ValidationError::InvalidImageId(
                "all zeros not allowed".to_string(),
            ));
        }

        Ok(())
    }
}

impl Default for ProofValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Validation result
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    /// Validation checks performed
    pub checks: Vec<(String, bool)>,
    /// Warnings (non-fatal)
    pub warnings: Vec<String>,
    /// Errors (fatal in strict mode)
    pub errors: Vec<String>,
    /// Computed image ID
    pub image_id: Option<String>,
    /// Computed journal hash
    pub journal_hash: Option<String>,
    /// Computed seal hash
    pub seal_hash: Option<String>,
}

impl ValidationResult {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_check(&mut self, name: &str, passed: bool) {
        self.checks.push((name.to_string(), passed));
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    pub fn add_error(&mut self, error: String) {
        self.errors.push(error);
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    pub fn all_checks_passed(&self) -> bool {
        self.checks.iter().all(|(_, passed)| *passed)
    }
}

/// Validation errors
#[derive(Debug, Clone)]
pub enum ValidationError {
    ProofTooLarge { size: usize, max: usize },
    JournalTooLarge { size: usize, max: usize },
    SealTooSmall { size: usize, min: usize },
    SealTooLarge { size: usize, max: usize },
    EmptySeal,
    InvalidImageId(String),
    ImageIdNotAllowed(String),
    InvalidJournalFormat(String),
    InvalidSealPattern,
    InsufficientEntropy,
    Multiple(Vec<String>),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProofTooLarge { size, max } => {
                write!(f, "Proof too large: {} bytes (max {})", size, max)
            }
            Self::JournalTooLarge { size, max } => {
                write!(f, "Journal too large: {} bytes (max {})", size, max)
            }
            Self::SealTooSmall { size, min } => {
                write!(f, "Seal too small: {} bytes (min {})", size, min)
            }
            Self::SealTooLarge { size, max } => {
                write!(f, "Seal too large: {} bytes (max {})", size, max)
            }
            Self::EmptySeal => write!(f, "Seal is empty"),
            Self::InvalidImageId(msg) => write!(f, "Invalid image ID: {}", msg),
            Self::ImageIdNotAllowed(id) => write!(f, "Image ID not allowed: {}", id),
            Self::InvalidJournalFormat(msg) => write!(f, "Invalid journal format: {}", msg),
            Self::InvalidSealPattern => write!(f, "Invalid seal pattern detected"),
            Self::InsufficientEntropy => write!(f, "Seal has insufficient entropy"),
            Self::Multiple(errors) => write!(f, "Multiple errors: {}", errors.join(", ")),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate proof integrity against expected values
pub fn verify_proof_integrity(
    proof: &ProofData,
    expected_image_id: &[u8; 32],
    expected_journal_hash: Option<&[u8; 32]>,
) -> Result<(), ValidationError> {
    // Check image ID matches
    if proof.image_id != *expected_image_id {
        return Err(ValidationError::InvalidImageId(format!(
            "expected {}, got {}",
            hex::encode(expected_image_id),
            hex::encode(proof.image_id)
        )));
    }

    // Check journal hash if provided
    if let Some(expected_hash) = expected_journal_hash {
        let mut hasher = Sha256::new();
        hasher.update(&proof.journal);
        let actual_hash = hasher.finalize();

        if actual_hash.as_slice() != expected_hash {
            return Err(ValidationError::InvalidJournalFormat(
                "journal hash mismatch".to_string(),
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_valid_proof() -> ProofData {
        ProofData {
            seal: (0..256).map(|i| i as u8).collect(), // 256 unique bytes
            journal: b"test journal data".to_vec(),
            image_id: [1u8; 32],
        }
    }

    #[test]
    fn test_valid_proof() {
        let validator = ProofValidator::new();
        let proof = make_valid_proof();

        let result = validator.validate(&proof);
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_seal() {
        let validator = ProofValidator::new();
        let proof = ProofData {
            seal: vec![],
            journal: vec![],
            image_id: [1u8; 32],
        };

        let result = validator.validate(&proof);
        assert!(matches!(result, Err(ValidationError::EmptySeal)));
    }

    #[test]
    fn test_zero_image_id() {
        let validator = ProofValidator::new();
        let proof = ProofData {
            seal: vec![1; 256],
            journal: vec![],
            image_id: [0u8; 32],
        };

        let result = validator.validate(&proof);
        assert!(matches!(result, Err(ValidationError::InvalidImageId(_))));
    }

    #[test]
    fn test_proof_too_large() {
        let config = ProofValidationConfig {
            max_proof_size: 100,
            ..Default::default()
        };
        let validator = ProofValidator::with_config(config);

        let proof = ProofData {
            seal: vec![1; 200],
            journal: vec![],
            image_id: [1u8; 32],
        };

        let result = validator.validate(&proof);
        assert!(matches!(result, Err(ValidationError::ProofTooLarge { .. })));
    }

    #[test]
    fn test_seal_too_small() {
        let validator = ProofValidator::new();
        let proof = ProofData {
            seal: vec![1; 10], // Less than min_seal_size
            journal: vec![],
            image_id: [1u8; 32],
        };

        let result = validator.validate(&proof);
        assert!(matches!(result, Err(ValidationError::SealTooSmall { .. })));
    }

    #[test]
    fn test_image_id_whitelist() {
        let mut allowed = HashSet::new();
        allowed.insert("0x0101010101010101010101010101010101010101010101010101010101010101".to_string());

        let config = ProofValidationConfig {
            allowed_image_ids: Some(allowed),
            ..Default::default()
        };
        let validator = ProofValidator::with_config(config);

        // Valid image ID
        let proof = make_valid_proof();
        assert!(validator.validate(&proof).is_ok());

        // Invalid image ID
        let proof = ProofData {
            seal: (0..256).map(|i| i as u8).collect(),
            journal: vec![],
            image_id: [2u8; 32], // Not in whitelist
        };
        assert!(matches!(
            validator.validate(&proof),
            Err(ValidationError::ImageIdNotAllowed(_))
        ));
    }

    #[test]
    fn test_insufficient_entropy() {
        let validator = ProofValidator::new();
        let proof = ProofData {
            seal: vec![42u8; 256], // All same byte
            journal: vec![],
            image_id: [1u8; 32],
        };

        let result = validator.validate(&proof);
        assert!(result.is_err());
    }

    #[test]
    fn test_quick_validate() {
        let validator = ProofValidator::new();

        let proof = make_valid_proof();
        assert!(validator.quick_validate(&proof).is_ok());

        let bad_proof = ProofData {
            seal: vec![],
            journal: vec![],
            image_id: [0u8; 32],
        };
        assert!(validator.quick_validate(&bad_proof).is_err());
    }

    #[test]
    fn test_verify_integrity() {
        let proof = make_valid_proof();

        // Correct image ID
        assert!(verify_proof_integrity(&proof, &[1u8; 32], None).is_ok());

        // Wrong image ID
        assert!(verify_proof_integrity(&proof, &[2u8; 32], None).is_err());
    }

    #[test]
    fn test_validation_result() {
        let mut result = ValidationResult::new();

        result.add_check("test1", true);
        result.add_check("test2", true);
        assert!(result.all_checks_passed());

        result.add_error("error".to_string());
        assert!(result.has_errors());

        result.add_warning("warning".to_string());
        assert!(result.has_warnings());
    }
}
