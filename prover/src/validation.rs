//! Input Validation and Sanitization
//!
//! Validates and sanitizes inputs before processing:
//! - Image ID format validation
//! - Input data size limits
//! - URL sanitization
//! - Address format validation
//!
//! ## Security
//!
//! All external inputs are validated before use to prevent:
//! - Buffer overflow attacks
//! - Injection attacks
//! - Resource exhaustion

use std::str::FromStr;
use thiserror::Error;

/// Validation errors
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Invalid image ID format: {0}")]
    InvalidImageId(String),

    #[error("Invalid address format: {0}")]
    InvalidAddress(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Input too large: {size} bytes (max: {max} bytes)")]
    InputTooLarge { size: usize, max: usize },

    #[error("Input too small: {size} bytes (min: {min} bytes)")]
    InputTooSmall { size: usize, min: usize },

    #[error("Invalid hex string: {0}")]
    InvalidHex(String),

    #[error("Invalid hash format: expected {expected} bytes, got {actual}")]
    InvalidHashLength { expected: usize, actual: usize },

    #[error("Empty input not allowed")]
    EmptyInput,

    #[error("Invalid character in input: {0}")]
    InvalidCharacter(char),

    #[error("Invalid CID format: {0}")]
    InvalidCid(String),
}

/// Validation configuration
#[derive(Debug, Clone)]
pub struct ValidationConfig {
    /// Maximum input size in bytes
    pub max_input_size: usize,
    /// Maximum URL length
    pub max_url_length: usize,
    /// Allowed URL schemes
    pub allowed_schemes: Vec<String>,
    /// Maximum image ID length
    pub max_image_id_length: usize,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            max_input_size: 100 * 1024 * 1024, // 100 MB
            max_url_length: 2048,
            allowed_schemes: vec!["https".to_string(), "ipfs".to_string(), "data".to_string()],
            max_image_id_length: 128,
        }
    }
}

/// Input validator
pub struct Validator {
    config: ValidationConfig,
}

impl Validator {
    /// Create a new validator with default config
    pub fn new() -> Self {
        Self {
            config: ValidationConfig::default(),
        }
    }

    /// Create a new validator with custom config
    pub fn with_config(config: ValidationConfig) -> Self {
        Self { config }
    }

    /// Validate an Ethereum address (0x-prefixed, 40 hex chars)
    pub fn validate_address(&self, address: &str) -> Result<String, ValidationError> {
        let address = address.trim();

        if address.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        // Must start with 0x
        if !address.starts_with("0x") && !address.starts_with("0X") {
            return Err(ValidationError::InvalidAddress(
                "must start with 0x".to_string(),
            ));
        }

        // Must be 42 characters total (0x + 40 hex)
        if address.len() != 42 {
            return Err(ValidationError::InvalidAddress(format!(
                "expected 42 characters, got {}",
                address.len()
            )));
        }

        // Validate hex characters
        let hex_part = &address[2..];
        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ValidationError::InvalidAddress(
                "contains non-hex characters".to_string(),
            ));
        }

        // Return lowercase normalized address
        Ok(address.to_lowercase())
    }

    /// Validate a bytes32 hash (0x-prefixed, 64 hex chars)
    pub fn validate_bytes32(&self, hash: &str) -> Result<String, ValidationError> {
        let hash = hash.trim();

        if hash.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        // Must start with 0x
        if !hash.starts_with("0x") && !hash.starts_with("0X") {
            return Err(ValidationError::InvalidHex(
                "must start with 0x".to_string(),
            ));
        }

        // Must be 66 characters total (0x + 64 hex)
        if hash.len() != 66 {
            return Err(ValidationError::InvalidHashLength {
                expected: 32,
                actual: (hash.len() - 2) / 2,
            });
        }

        // Validate hex characters
        let hex_part = &hash[2..];
        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ValidationError::InvalidHex(
                "contains non-hex characters".to_string(),
            ));
        }

        Ok(hash.to_lowercase())
    }

    /// Validate an image ID (RISC Zero image ID format)
    pub fn validate_image_id(&self, image_id: &str) -> Result<String, ValidationError> {
        let image_id = image_id.trim();

        if image_id.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        if image_id.len() > self.config.max_image_id_length {
            return Err(ValidationError::InvalidImageId(format!(
                "too long: {} chars (max: {})",
                image_id.len(),
                self.config.max_image_id_length
            )));
        }

        // Image ID should be hex (with or without 0x prefix)
        let hex_part = if image_id.starts_with("0x") || image_id.starts_with("0X") {
            &image_id[2..]
        } else {
            image_id
        };

        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ValidationError::InvalidImageId(
                "contains non-hex characters".to_string(),
            ));
        }

        // Should be 64 hex chars (32 bytes)
        if hex_part.len() != 64 {
            return Err(ValidationError::InvalidImageId(format!(
                "expected 64 hex chars, got {}",
                hex_part.len()
            )));
        }

        Ok(image_id.to_lowercase())
    }

    /// Validate input data size
    pub fn validate_input_size(&self, data: &[u8]) -> Result<(), ValidationError> {
        if data.len() > self.config.max_input_size {
            return Err(ValidationError::InputTooLarge {
                size: data.len(),
                max: self.config.max_input_size,
            });
        }
        Ok(())
    }

    /// Validate a URL
    pub fn validate_url(&self, url: &str) -> Result<String, ValidationError> {
        let url = url.trim();

        if url.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        if url.len() > self.config.max_url_length {
            return Err(ValidationError::InvalidUrl(format!(
                "too long: {} chars (max: {})",
                url.len(),
                self.config.max_url_length
            )));
        }

        // Check for valid scheme
        let has_valid_scheme = self
            .config
            .allowed_schemes
            .iter()
            .any(|scheme| url.starts_with(&format!("{}:", scheme)));

        if !has_valid_scheme {
            return Err(ValidationError::InvalidUrl(format!(
                "scheme not allowed. Allowed: {:?}",
                self.config.allowed_schemes
            )));
        }

        // Basic URL validation - no control characters
        if url.chars().any(|c| c.is_control()) {
            return Err(ValidationError::InvalidUrl(
                "contains control characters".to_string(),
            ));
        }

        Ok(url.to_string())
    }

    /// Validate an IPFS CID
    pub fn validate_cid(&self, cid: &str) -> Result<String, ValidationError> {
        let cid = cid.trim();

        if cid.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        // CIDv0: starts with Qm, 46 chars, base58
        // CIDv1: starts with b, variable length, base32/base58
        if cid.starts_with("Qm") {
            if cid.len() != 46 {
                return Err(ValidationError::InvalidCid(format!(
                    "CIDv0 should be 46 chars, got {}",
                    cid.len()
                )));
            }
            // Base58 character set
            if !cid.chars().all(|c| {
                c.is_ascii_alphanumeric() && c != '0' && c != 'O' && c != 'I' && c != 'l'
            }) {
                return Err(ValidationError::InvalidCid(
                    "invalid base58 characters".to_string(),
                ));
            }
        } else if cid.starts_with("bafy") || cid.starts_with("bafk") {
            // CIDv1 with base32
            if cid.len() < 50 {
                return Err(ValidationError::InvalidCid(
                    "CIDv1 too short".to_string(),
                ));
            }
            // Base32 lowercase
            if !cid.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
                return Err(ValidationError::InvalidCid(
                    "invalid base32 characters".to_string(),
                ));
            }
        } else {
            return Err(ValidationError::InvalidCid(
                "unknown CID format".to_string(),
            ));
        }

        Ok(cid.to_string())
    }

    /// Validate a private key (for config validation, not storage)
    pub fn validate_private_key(&self, key: &str) -> Result<(), ValidationError> {
        let key = key.trim();

        if key.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        // Must start with 0x
        if !key.starts_with("0x") && !key.starts_with("0X") {
            return Err(ValidationError::InvalidHex(
                "must start with 0x".to_string(),
            ));
        }

        // Must be 66 characters (0x + 64 hex)
        if key.len() != 66 {
            return Err(ValidationError::InvalidHex(format!(
                "expected 66 characters, got {}",
                key.len()
            )));
        }

        // Validate hex
        let hex_part = &key[2..];
        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ValidationError::InvalidHex(
                "contains non-hex characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Sanitize a string for logging (remove sensitive data patterns)
    pub fn sanitize_for_logging(&self, input: &str) -> String {
        let mut result = input.to_string();

        // Mask private keys
        let key_pattern = regex_lite::Regex::new(r"0x[a-fA-F0-9]{64}").unwrap();
        result = key_pattern.replace_all(&result, "0x[REDACTED]").to_string();

        // Mask API keys (common patterns)
        let api_pattern = regex_lite::Regex::new(r"(?i)(api[_-]?key|apikey|secret)[=:]\s*[a-zA-Z0-9_-]+").unwrap();
        result = api_pattern.replace_all(&result, "$1=[REDACTED]").to_string();

        result
    }
}

impl Default for Validator {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate request ID (u64)
pub fn validate_request_id(id: u64) -> Result<u64, ValidationError> {
    if id == 0 {
        return Err(ValidationError::EmptyInput);
    }
    Ok(id)
}

/// Validate tip amount (must be reasonable)
pub fn validate_tip(tip: u128, max_tip: u128) -> Result<u128, ValidationError> {
    if tip > max_tip {
        return Err(ValidationError::InputTooLarge {
            size: tip as usize,
            max: max_tip as usize,
        });
    }
    Ok(tip)
}

/// Quick validation helpers
pub mod quick {
    use super::*;

    /// Check if string is valid hex
    pub fn is_hex(s: &str) -> bool {
        let s = s.strip_prefix("0x").unwrap_or(s);
        !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Check if string is valid address
    pub fn is_address(s: &str) -> bool {
        s.len() == 42
            && (s.starts_with("0x") || s.starts_with("0X"))
            && s[2..].chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Check if string is valid bytes32
    pub fn is_bytes32(s: &str) -> bool {
        s.len() == 66
            && (s.starts_with("0x") || s.starts_with("0X"))
            && s[2..].chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Check if URL has allowed scheme
    pub fn has_allowed_scheme(url: &str, schemes: &[&str]) -> bool {
        schemes.iter().any(|s| url.starts_with(&format!("{}:", s)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_address() {
        let v = Validator::new();

        // Valid addresses
        assert!(v.validate_address("0x1234567890abcdef1234567890abcdef12345678").is_ok());
        assert!(v.validate_address("0xABCDEF1234567890ABCDEF1234567890ABCDEF12").is_ok());

        // Invalid addresses
        assert!(v.validate_address("").is_err());
        assert!(v.validate_address("1234567890abcdef1234567890abcdef12345678").is_err());
        assert!(v.validate_address("0x123").is_err());
        assert!(v.validate_address("0xGGGG567890abcdef1234567890abcdef12345678").is_err());
    }

    #[test]
    fn test_validate_bytes32() {
        let v = Validator::new();

        // Valid bytes32
        let valid = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        assert!(v.validate_bytes32(valid).is_ok());

        // Invalid
        assert!(v.validate_bytes32("").is_err());
        assert!(v.validate_bytes32("0x1234").is_err());
        assert!(v.validate_bytes32("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef").is_err());
    }

    #[test]
    fn test_validate_image_id() {
        let v = Validator::new();

        // Valid image ID (64 hex chars)
        let valid = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        assert!(v.validate_image_id(valid).is_ok());
        assert!(v.validate_image_id(&format!("0x{}", valid)).is_ok());

        // Invalid
        assert!(v.validate_image_id("").is_err());
        assert!(v.validate_image_id("1234").is_err());
        assert!(v.validate_image_id("zzzz567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef").is_err());
    }

    #[test]
    fn test_validate_url() {
        let v = Validator::new();

        // Valid URLs
        assert!(v.validate_url("https://example.com/data").is_ok());
        assert!(v.validate_url("ipfs://QmTest").is_ok());
        assert!(v.validate_url("data:application/octet-stream;base64,dGVzdA==").is_ok());

        // Invalid URLs
        assert!(v.validate_url("").is_err());
        assert!(v.validate_url("http://example.com").is_err()); // http not allowed
        assert!(v.validate_url("ftp://example.com").is_err());
    }

    #[test]
    fn test_validate_cid() {
        let v = Validator::new();

        // Valid CIDv0
        assert!(v.validate_cid("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG").is_ok());

        // Valid CIDv1
        assert!(v.validate_cid("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi").is_ok());

        // Invalid
        assert!(v.validate_cid("").is_err());
        assert!(v.validate_cid("invalid").is_err());
        assert!(v.validate_cid("Qm123").is_err()); // Too short
    }

    #[test]
    fn test_validate_input_size() {
        let v = Validator::new();

        // Valid size
        assert!(v.validate_input_size(&vec![0u8; 1024]).is_ok());

        // Too large
        let large = vec![0u8; 200 * 1024 * 1024];
        assert!(v.validate_input_size(&large).is_err());
    }

    #[test]
    fn test_validate_private_key() {
        let v = Validator::new();

        // Valid
        let valid = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        assert!(v.validate_private_key(valid).is_ok());

        // Invalid
        assert!(v.validate_private_key("").is_err());
        assert!(v.validate_private_key("0x123").is_err());
    }

    #[test]
    fn test_sanitize_for_logging() {
        let v = Validator::new();

        let input = "key=0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let sanitized = v.sanitize_for_logging(input);
        assert!(!sanitized.contains("1234567890abcdef"));
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn test_quick_helpers() {
        assert!(quick::is_hex("0xabcdef"));
        assert!(quick::is_hex("123abc"));
        assert!(!quick::is_hex("0xghij"));

        assert!(quick::is_address("0x1234567890abcdef1234567890abcdef12345678"));
        assert!(!quick::is_address("0x123"));

        assert!(quick::is_bytes32("0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"));

        assert!(quick::has_allowed_scheme("https://test.com", &["https", "http"]));
        assert!(!quick::has_allowed_scheme("ftp://test.com", &["https", "http"]));
    }

    #[test]
    fn test_validate_request_id() {
        assert!(validate_request_id(1).is_ok());
        assert!(validate_request_id(0).is_err());
    }
}
