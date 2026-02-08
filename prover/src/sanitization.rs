//! Input Sanitization and Security Hardening
//!
//! Provides comprehensive input validation and sanitization to prevent:
//! - Injection attacks
//! - Path traversal
//! - Integer overflow
//! - Malformed data

use std::collections::HashSet;

/// Security configuration for input validation
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// Maximum input size in bytes
    pub max_input_size: usize,
    /// Maximum string length
    pub max_string_length: usize,
    /// Maximum array/list size
    pub max_array_size: usize,
    /// Maximum recursion depth for nested structures
    pub max_recursion_depth: usize,
    /// Allowed URL schemes
    pub allowed_schemes: HashSet<String>,
    /// Blocked file extensions
    pub blocked_extensions: HashSet<String>,
    /// Enable strict mode (reject anything suspicious)
    pub strict_mode: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            max_input_size: 10 * 1024 * 1024, // 10MB
            max_string_length: 10_000,
            max_array_size: 10_000,
            max_recursion_depth: 32,
            allowed_schemes: ["https", "ipfs", "ar"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            blocked_extensions: ["exe", "dll", "so", "sh", "bat", "cmd", "ps1"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            strict_mode: true,
        }
    }
}

/// Input sanitizer
pub struct Sanitizer {
    config: SecurityConfig,
}

impl Sanitizer {
    /// Create with default config
    pub fn new() -> Self {
        Self {
            config: SecurityConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(config: SecurityConfig) -> Self {
        Self { config }
    }

    /// Sanitize a hex string (address, hash, etc.)
    pub fn sanitize_hex(&self, input: &str) -> Result<String, SanitizationError> {
        let cleaned = input.trim();

        // Check length
        if cleaned.len() > self.config.max_string_length {
            return Err(SanitizationError::TooLong {
                max: self.config.max_string_length,
                actual: cleaned.len(),
            });
        }

        // Remove 0x prefix if present
        let hex_part = cleaned.strip_prefix("0x").unwrap_or(cleaned);

        // Validate hex characters only
        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(SanitizationError::InvalidHex(
                "contains non-hex characters".to_string(),
            ));
        }

        // Return normalized form
        Ok(format!("0x{}", hex_part.to_lowercase()))
    }

    /// Sanitize an Ethereum address
    pub fn sanitize_address(&self, input: &str) -> Result<String, SanitizationError> {
        let hex = self.sanitize_hex(input)?;

        // Address must be 42 chars (0x + 40 hex chars)
        if hex.len() != 42 {
            return Err(SanitizationError::InvalidAddress(format!(
                "expected 42 chars, got {}",
                hex.len()
            )));
        }

        Ok(hex)
    }

    /// Sanitize a bytes32 value (image ID, hash, etc.)
    pub fn sanitize_bytes32(&self, input: &str) -> Result<String, SanitizationError> {
        let hex = self.sanitize_hex(input)?;

        // bytes32 must be 66 chars (0x + 64 hex chars)
        if hex.len() != 66 {
            return Err(SanitizationError::InvalidBytes32(format!(
                "expected 66 chars, got {}",
                hex.len()
            )));
        }

        Ok(hex)
    }

    /// Sanitize a URL
    pub fn sanitize_url(&self, input: &str) -> Result<String, SanitizationError> {
        let cleaned = input.trim();

        // Check length
        if cleaned.len() > self.config.max_string_length {
            return Err(SanitizationError::TooLong {
                max: self.config.max_string_length,
                actual: cleaned.len(),
            });
        }

        // Check for dangerous characters
        if cleaned.contains("..") || cleaned.contains('\0') {
            return Err(SanitizationError::DangerousContent(
                "path traversal or null byte detected".to_string(),
            ));
        }

        // Extract and validate scheme
        let scheme = cleaned
            .split("://")
            .next()
            .ok_or_else(|| SanitizationError::InvalidUrl("no scheme found".to_string()))?
            .to_lowercase();

        if !self.config.allowed_schemes.contains(&scheme) {
            return Err(SanitizationError::DisallowedScheme(scheme));
        }

        // Check for blocked extensions
        let path = cleaned.split('?').next().unwrap_or(cleaned);
        if let Some(ext) = path.rsplit('.').next() {
            if self.config.blocked_extensions.contains(&ext.to_lowercase()) {
                return Err(SanitizationError::BlockedExtension(ext.to_string()));
            }
        }

        // URL encode dangerous characters
        let sanitized = cleaned
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace('"', "%22")
            .replace('\'', "%27");

        Ok(sanitized)
    }

    /// Sanitize an IPFS CID
    pub fn sanitize_cid(&self, input: &str) -> Result<String, SanitizationError> {
        let cleaned = input.trim();

        // Check length (CIDv0: 46 chars, CIDv1: variable but reasonable)
        if cleaned.len() < 46 || cleaned.len() > 100 {
            return Err(SanitizationError::InvalidCid(format!(
                "invalid length: {}",
                cleaned.len()
            )));
        }

        // CIDv0 starts with Qm, CIDv1 starts with b
        if !cleaned.starts_with("Qm") && !cleaned.starts_with("b") {
            return Err(SanitizationError::InvalidCid(
                "must start with Qm or b".to_string(),
            ));
        }

        // Only allow base58 or base32 characters
        let valid_chars = cleaned.chars().all(|c| {
            c.is_ascii_alphanumeric() && c != '0' && c != 'O' && c != 'I' && c != 'l'
                || c.is_ascii_lowercase()
        });

        if !valid_chars {
            return Err(SanitizationError::InvalidCid(
                "contains invalid characters".to_string(),
            ));
        }

        Ok(cleaned.to_string())
    }

    /// Sanitize binary data
    pub fn sanitize_bytes(&self, input: &[u8]) -> Result<Vec<u8>, SanitizationError> {
        // Check size
        if input.len() > self.config.max_input_size {
            return Err(SanitizationError::TooLarge {
                max: self.config.max_input_size,
                actual: input.len(),
            });
        }

        // In strict mode, check for executable signatures
        if self.config.strict_mode {
            self.check_executable_signature(input)?;
        }

        Ok(input.to_vec())
    }

    /// Sanitize a numeric value
    pub fn sanitize_u64(&self, input: &str) -> Result<u64, SanitizationError> {
        let cleaned = input.trim();

        // Check for negative sign
        if cleaned.starts_with('-') {
            return Err(SanitizationError::NegativeNumber);
        }

        // Parse with overflow protection
        cleaned
            .parse::<u64>()
            .map_err(|e| SanitizationError::InvalidNumber(e.to_string()))
    }

    /// Sanitize a request ID
    pub fn sanitize_request_id(&self, input: u64) -> Result<u64, SanitizationError> {
        // Request ID must be non-zero
        if input == 0 {
            return Err(SanitizationError::InvalidRequestId(
                "request ID cannot be zero".to_string(),
            ));
        }

        Ok(input)
    }

    /// Sanitize JSON input
    pub fn sanitize_json(&self, input: &str) -> Result<serde_json::Value, SanitizationError> {
        // Check size
        if input.len() > self.config.max_input_size {
            return Err(SanitizationError::TooLarge {
                max: self.config.max_input_size,
                actual: input.len(),
            });
        }

        // Parse JSON
        let value: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| SanitizationError::InvalidJson(e.to_string()))?;

        // Validate recursion depth and array sizes
        self.validate_json_value(&value, 0)?;

        Ok(value)
    }

    fn validate_json_value(
        &self,
        value: &serde_json::Value,
        depth: usize,
    ) -> Result<(), SanitizationError> {
        if depth > self.config.max_recursion_depth {
            return Err(SanitizationError::TooDeep {
                max: self.config.max_recursion_depth,
            });
        }

        match value {
            serde_json::Value::Array(arr) => {
                if arr.len() > self.config.max_array_size {
                    return Err(SanitizationError::ArrayTooLarge {
                        max: self.config.max_array_size,
                        actual: arr.len(),
                    });
                }
                for item in arr {
                    self.validate_json_value(item, depth + 1)?;
                }
            }
            serde_json::Value::Object(obj) => {
                if obj.len() > self.config.max_array_size {
                    return Err(SanitizationError::ArrayTooLarge {
                        max: self.config.max_array_size,
                        actual: obj.len(),
                    });
                }
                for (key, val) in obj {
                    if key.len() > self.config.max_string_length {
                        return Err(SanitizationError::TooLong {
                            max: self.config.max_string_length,
                            actual: key.len(),
                        });
                    }
                    self.validate_json_value(val, depth + 1)?;
                }
            }
            serde_json::Value::String(s) => {
                if s.len() > self.config.max_string_length {
                    return Err(SanitizationError::TooLong {
                        max: self.config.max_string_length,
                        actual: s.len(),
                    });
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn check_executable_signature(&self, data: &[u8]) -> Result<(), SanitizationError> {
        if data.len() < 4 {
            return Ok(());
        }

        // Check for common executable signatures
        let signatures: &[(&[u8], &str)] = &[
            (b"\x7fELF", "ELF executable"),
            (b"MZ", "Windows executable"),
            (b"\xca\xfe\xba\xbe", "Mach-O executable"),
            (b"\xfe\xed\xfa\xce", "Mach-O executable"),
            (b"\xfe\xed\xfa\xcf", "Mach-O 64-bit executable"),
            (b"PK\x03\x04", "ZIP/JAR archive"),
            (b"#!/", "Script file"),
        ];

        for (sig, name) in signatures {
            if data.starts_with(sig) {
                return Err(SanitizationError::ExecutableContent(name.to_string()));
            }
        }

        Ok(())
    }

    /// Sanitize for logging (remove sensitive data)
    pub fn sanitize_for_log(&self, input: &str) -> String {
        let mut result = input.to_string();

        // Mask private keys
        let key_pattern = regex_lite::Regex::new(r"0x[a-fA-F0-9]{64}").unwrap();
        result = key_pattern
            .replace_all(&result, "0x[REDACTED_KEY]")
            .to_string();

        // Mask API keys
        let api_pattern = regex_lite::Regex::new(r"(?i)(api[_-]?key|bearer|token)[=:\s]+\S+").unwrap();
        result = api_pattern
            .replace_all(&result, "$1=[REDACTED]")
            .to_string();

        // Mask passwords
        let pwd_pattern = regex_lite::Regex::new(r"(?i)(password|pwd|secret)[=:\s]+\S+").unwrap();
        result = pwd_pattern
            .replace_all(&result, "$1=[REDACTED]")
            .to_string();

        result
    }
}

impl Default for Sanitizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Sanitization errors
#[derive(Debug, Clone)]
pub enum SanitizationError {
    TooLong { max: usize, actual: usize },
    TooLarge { max: usize, actual: usize },
    TooDeep { max: usize },
    ArrayTooLarge { max: usize, actual: usize },
    InvalidHex(String),
    InvalidAddress(String),
    InvalidBytes32(String),
    InvalidUrl(String),
    InvalidCid(String),
    InvalidNumber(String),
    InvalidJson(String),
    InvalidRequestId(String),
    NegativeNumber,
    DisallowedScheme(String),
    BlockedExtension(String),
    DangerousContent(String),
    ExecutableContent(String),
}

impl std::fmt::Display for SanitizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong { max, actual } => {
                write!(f, "Input too long: {} chars (max {})", actual, max)
            }
            Self::TooLarge { max, actual } => {
                write!(f, "Input too large: {} bytes (max {})", actual, max)
            }
            Self::TooDeep { max } => write!(f, "Recursion too deep (max {})", max),
            Self::ArrayTooLarge { max, actual } => {
                write!(f, "Array too large: {} items (max {})", actual, max)
            }
            Self::InvalidHex(msg) => write!(f, "Invalid hex: {}", msg),
            Self::InvalidAddress(msg) => write!(f, "Invalid address: {}", msg),
            Self::InvalidBytes32(msg) => write!(f, "Invalid bytes32: {}", msg),
            Self::InvalidUrl(msg) => write!(f, "Invalid URL: {}", msg),
            Self::InvalidCid(msg) => write!(f, "Invalid CID: {}", msg),
            Self::InvalidNumber(msg) => write!(f, "Invalid number: {}", msg),
            Self::InvalidJson(msg) => write!(f, "Invalid JSON: {}", msg),
            Self::InvalidRequestId(msg) => write!(f, "Invalid request ID: {}", msg),
            Self::NegativeNumber => write!(f, "Negative numbers not allowed"),
            Self::DisallowedScheme(s) => write!(f, "URL scheme not allowed: {}", s),
            Self::BlockedExtension(e) => write!(f, "File extension blocked: {}", e),
            Self::DangerousContent(msg) => write!(f, "Dangerous content detected: {}", msg),
            Self::ExecutableContent(t) => write!(f, "Executable content detected: {}", t),
        }
    }
}

impl std::error::Error for SanitizationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_hex() {
        let s = Sanitizer::new();

        assert_eq!(s.sanitize_hex("0xABCD").unwrap(), "0xabcd");
        assert_eq!(s.sanitize_hex("abcd").unwrap(), "0xabcd");
        assert_eq!(s.sanitize_hex("  0xABCD  ").unwrap(), "0xabcd");

        assert!(s.sanitize_hex("0xGHIJ").is_err());
        assert!(s.sanitize_hex("not hex").is_err());
    }

    #[test]
    fn test_sanitize_address() {
        let s = Sanitizer::new();

        let valid = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
        assert!(s.sanitize_address(valid).is_ok());

        assert!(s.sanitize_address("0x123").is_err()); // Too short
        assert!(s.sanitize_address("not an address").is_err());
    }

    #[test]
    fn test_sanitize_bytes32() {
        let s = Sanitizer::new();

        let valid = "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(s.sanitize_bytes32(valid).is_ok());

        assert!(s.sanitize_bytes32("0x123").is_err()); // Too short
    }

    #[test]
    fn test_sanitize_url() {
        let s = Sanitizer::new();

        assert!(s.sanitize_url("https://example.com/path").is_ok());
        assert!(s.sanitize_url("ipfs://QmTest").is_ok());

        assert!(s.sanitize_url("http://example.com").is_err()); // HTTP not allowed
        assert!(s.sanitize_url("file:///etc/passwd").is_err()); // file:// not allowed
        assert!(s.sanitize_url("https://example.com/../etc/passwd").is_err()); // Path traversal
        assert!(s.sanitize_url("https://example.com/file.exe").is_err()); // Blocked extension
    }

    #[test]
    fn test_sanitize_cid() {
        let s = Sanitizer::new();

        // Valid CIDv0
        assert!(s.sanitize_cid("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG").is_ok());

        // Valid CIDv1
        assert!(s.sanitize_cid("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi").is_ok());

        assert!(s.sanitize_cid("invalid").is_err());
        assert!(s.sanitize_cid("").is_err());
    }

    #[test]
    fn test_sanitize_bytes() {
        let s = Sanitizer::new();

        assert!(s.sanitize_bytes(b"normal data").is_ok());

        // ELF executable signature
        assert!(s.sanitize_bytes(b"\x7fELF...").is_err());

        // Windows executable
        assert!(s.sanitize_bytes(b"MZ...").is_err());
    }

    #[test]
    fn test_sanitize_u64() {
        let s = Sanitizer::new();

        assert_eq!(s.sanitize_u64("123").unwrap(), 123);
        assert_eq!(s.sanitize_u64("  456  ").unwrap(), 456);

        assert!(s.sanitize_u64("-1").is_err());
        assert!(s.sanitize_u64("not a number").is_err());
        assert!(s.sanitize_u64("99999999999999999999999").is_err()); // Overflow
    }

    #[test]
    fn test_sanitize_json() {
        let s = Sanitizer::new();

        assert!(s.sanitize_json(r#"{"key": "value"}"#).is_ok());
        assert!(s.sanitize_json(r#"[1, 2, 3]"#).is_ok());

        assert!(s.sanitize_json("not json").is_err());
    }

    #[test]
    fn test_json_depth_limit() {
        let config = SecurityConfig {
            max_recursion_depth: 3,
            ..Default::default()
        };
        let s = Sanitizer::with_config(config);

        // Depth 2 - OK
        assert!(s.sanitize_json(r#"{"a": {"b": 1}}"#).is_ok());

        // Depth 4 - Too deep
        assert!(s.sanitize_json(r#"{"a": {"b": {"c": {"d": 1}}}}"#).is_err());
    }

    #[test]
    fn test_sanitize_for_log() {
        let s = Sanitizer::new();

        let input = "key=0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let output = s.sanitize_for_log(input);
        assert!(output.contains("[REDACTED_KEY]"));
        assert!(!output.contains("0123456789"));

        let input = "api_key=secret123";
        let output = s.sanitize_for_log(input);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("secret123"));
    }

    #[test]
    fn test_request_id() {
        let s = Sanitizer::new();

        assert!(s.sanitize_request_id(1).is_ok());
        assert!(s.sanitize_request_id(u64::MAX).is_ok());
        assert!(s.sanitize_request_id(0).is_err());
    }
}
