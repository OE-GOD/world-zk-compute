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

use thiserror::Error;

/// Validation errors
#[derive(Debug, Error)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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

}

impl Default for Validator {
    fn default() -> Self {
        Self::new()
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

}
