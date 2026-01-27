//! Structured error codes and messages for the World ZK Compute API.
//!
//! Error codes follow the format: `WZK-XXXX` where:
//! - WZK = World ZK Compute prefix
//! - First digit: Category (1=Client, 2=Server, 3=Proof, 4=Contract, 5=Network)
//! - Last 3 digits: Specific error

use serde::{Deserialize, Serialize};
use std::fmt;

// ============================================================================
// Error Codes
// ============================================================================

/// Structured error code enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum ErrorCode {
    // 1xxx - Client Errors (bad request, validation, auth)
    InvalidRequest,           // WZK-1000
    InvalidImageId,           // WZK-1001
    InvalidInputHash,         // WZK-1002
    InvalidInputUrl,          // WZK-1003
    InvalidRequestId,         // WZK-1004
    InvalidProof,             // WZK-1005
    InvalidSignature,         // WZK-1006
    InputTooLarge,            // WZK-1010
    InputTooSmall,            // WZK-1011
    RequestExpired,           // WZK-1020
    RequestNotFound,          // WZK-1021
    RequestAlreadyClaimed,    // WZK-1022
    RequestNotClaimed,        // WZK-1023
    RequestAlreadyCompleted,  // WZK-1024
    Unauthorized,             // WZK-1030
    Forbidden,                // WZK-1031
    RateLimited,              // WZK-1040
    QuotaExceeded,            // WZK-1041

    // 2xxx - Server Errors (internal failures)
    InternalError,            // WZK-2000
    DatabaseError,            // WZK-2001
    CacheError,               // WZK-2002
    ConfigurationError,       // WZK-2003
    ServiceUnavailable,       // WZK-2010
    ServiceOverloaded,        // WZK-2011
    MaintenanceMode,          // WZK-2012
    ShuttingDown,             // WZK-2013

    // 3xxx - Proof Errors (proving failures)
    ProofGenerationFailed,    // WZK-3000
    ProofVerificationFailed,  // WZK-3001
    ProofTimeout,             // WZK-3002
    ProofCancelled,           // WZK-3003
    GuestProgramError,        // WZK-3010
    GuestProgramPanic,        // WZK-3011
    GuestProgramOOM,          // WZK-3012
    GuestProgramTimeout,      // WZK-3013
    ImageNotFound,            // WZK-3020
    ImageNotRegistered,       // WZK-3021
    ImageInactive,            // WZK-3022
    BonsaiError,              // WZK-3030
    BonsaiTimeout,            // WZK-3031
    BonsaiQuotaExceeded,      // WZK-3032

    // 4xxx - Contract/Chain Errors
    ContractError,            // WZK-4000
    TransactionFailed,        // WZK-4001
    TransactionReverted,      // WZK-4002
    InsufficientFunds,        // WZK-4003
    GasEstimationFailed,      // WZK-4004
    NonceError,               // WZK-4005
    ChainUnavailable,         // WZK-4010
    ChainReorg,               // WZK-4011
    WrongChain,               // WZK-4012

    // 5xxx - Network/External Errors
    NetworkError,             // WZK-5000
    ConnectionTimeout,        // WZK-5001
    ConnectionRefused,        // WZK-5002
    DnsError,                 // WZK-5003
    TlsError,                 // WZK-5004
    IpfsError,                // WZK-5010
    IpfsFetchFailed,          // WZK-5011
    IpfsUploadFailed,         // WZK-5012
    IpfsTimeout,              // WZK-5013
    ExternalServiceError,     // WZK-5020
}

impl ErrorCode {
    /// Get the numeric code
    pub fn code(&self) -> u16 {
        match self {
            // 1xxx - Client Errors
            Self::InvalidRequest => 1000,
            Self::InvalidImageId => 1001,
            Self::InvalidInputHash => 1002,
            Self::InvalidInputUrl => 1003,
            Self::InvalidRequestId => 1004,
            Self::InvalidProof => 1005,
            Self::InvalidSignature => 1006,
            Self::InputTooLarge => 1010,
            Self::InputTooSmall => 1011,
            Self::RequestExpired => 1020,
            Self::RequestNotFound => 1021,
            Self::RequestAlreadyClaimed => 1022,
            Self::RequestNotClaimed => 1023,
            Self::RequestAlreadyCompleted => 1024,
            Self::Unauthorized => 1030,
            Self::Forbidden => 1031,
            Self::RateLimited => 1040,
            Self::QuotaExceeded => 1041,

            // 2xxx - Server Errors
            Self::InternalError => 2000,
            Self::DatabaseError => 2001,
            Self::CacheError => 2002,
            Self::ConfigurationError => 2003,
            Self::ServiceUnavailable => 2010,
            Self::ServiceOverloaded => 2011,
            Self::MaintenanceMode => 2012,
            Self::ShuttingDown => 2013,

            // 3xxx - Proof Errors
            Self::ProofGenerationFailed => 3000,
            Self::ProofVerificationFailed => 3001,
            Self::ProofTimeout => 3002,
            Self::ProofCancelled => 3003,
            Self::GuestProgramError => 3010,
            Self::GuestProgramPanic => 3011,
            Self::GuestProgramOOM => 3012,
            Self::GuestProgramTimeout => 3013,
            Self::ImageNotFound => 3020,
            Self::ImageNotRegistered => 3021,
            Self::ImageInactive => 3022,
            Self::BonsaiError => 3030,
            Self::BonsaiTimeout => 3031,
            Self::BonsaiQuotaExceeded => 3032,

            // 4xxx - Contract Errors
            Self::ContractError => 4000,
            Self::TransactionFailed => 4001,
            Self::TransactionReverted => 4002,
            Self::InsufficientFunds => 4003,
            Self::GasEstimationFailed => 4004,
            Self::NonceError => 4005,
            Self::ChainUnavailable => 4010,
            Self::ChainReorg => 4011,
            Self::WrongChain => 4012,

            // 5xxx - Network Errors
            Self::NetworkError => 5000,
            Self::ConnectionTimeout => 5001,
            Self::ConnectionRefused => 5002,
            Self::DnsError => 5003,
            Self::TlsError => 5004,
            Self::IpfsError => 5010,
            Self::IpfsFetchFailed => 5011,
            Self::IpfsUploadFailed => 5012,
            Self::IpfsTimeout => 5013,
            Self::ExternalServiceError => 5020,
        }
    }

    /// Get the string code (e.g., "WZK-1000")
    pub fn code_string(&self) -> String {
        format!("WZK-{:04}", self.code())
    }

    /// Get the default message for this error
    pub fn default_message(&self) -> &'static str {
        match self {
            // 1xxx - Client Errors
            Self::InvalidRequest => "Invalid request format or parameters",
            Self::InvalidImageId => "Invalid image ID format (expected 32-byte hex)",
            Self::InvalidInputHash => "Invalid input hash format (expected 32-byte hex)",
            Self::InvalidInputUrl => "Invalid input URL format",
            Self::InvalidRequestId => "Invalid request ID",
            Self::InvalidProof => "Invalid proof format or data",
            Self::InvalidSignature => "Invalid cryptographic signature",
            Self::InputTooLarge => "Input data exceeds maximum allowed size",
            Self::InputTooSmall => "Input data is below minimum required size",
            Self::RequestExpired => "Request has expired",
            Self::RequestNotFound => "Request not found",
            Self::RequestAlreadyClaimed => "Request has already been claimed by another prover",
            Self::RequestNotClaimed => "Request has not been claimed yet",
            Self::RequestAlreadyCompleted => "Request has already been completed",
            Self::Unauthorized => "Authentication required",
            Self::Forbidden => "Access denied",
            Self::RateLimited => "Too many requests, please slow down",
            Self::QuotaExceeded => "Usage quota exceeded",

            // 2xxx - Server Errors
            Self::InternalError => "Internal server error",
            Self::DatabaseError => "Database operation failed",
            Self::CacheError => "Cache operation failed",
            Self::ConfigurationError => "Server configuration error",
            Self::ServiceUnavailable => "Service temporarily unavailable",
            Self::ServiceOverloaded => "Service is overloaded, try again later",
            Self::MaintenanceMode => "Service is under maintenance",
            Self::ShuttingDown => "Service is shutting down",

            // 3xxx - Proof Errors
            Self::ProofGenerationFailed => "Failed to generate proof",
            Self::ProofVerificationFailed => "Proof verification failed",
            Self::ProofTimeout => "Proof generation timed out",
            Self::ProofCancelled => "Proof generation was cancelled",
            Self::GuestProgramError => "Guest program execution error",
            Self::GuestProgramPanic => "Guest program panicked",
            Self::GuestProgramOOM => "Guest program ran out of memory",
            Self::GuestProgramTimeout => "Guest program execution timed out",
            Self::ImageNotFound => "Program image not found",
            Self::ImageNotRegistered => "Program image is not registered",
            Self::ImageInactive => "Program image is inactive",
            Self::BonsaiError => "Bonsai proving service error",
            Self::BonsaiTimeout => "Bonsai proving service timed out",
            Self::BonsaiQuotaExceeded => "Bonsai quota exceeded",

            // 4xxx - Contract Errors
            Self::ContractError => "Smart contract error",
            Self::TransactionFailed => "Transaction failed",
            Self::TransactionReverted => "Transaction reverted",
            Self::InsufficientFunds => "Insufficient funds for transaction",
            Self::GasEstimationFailed => "Gas estimation failed",
            Self::NonceError => "Transaction nonce error",
            Self::ChainUnavailable => "Blockchain network unavailable",
            Self::ChainReorg => "Chain reorganization detected",
            Self::WrongChain => "Connected to wrong chain",

            // 5xxx - Network Errors
            Self::NetworkError => "Network error",
            Self::ConnectionTimeout => "Connection timed out",
            Self::ConnectionRefused => "Connection refused",
            Self::DnsError => "DNS resolution failed",
            Self::TlsError => "TLS/SSL error",
            Self::IpfsError => "IPFS error",
            Self::IpfsFetchFailed => "Failed to fetch from IPFS",
            Self::IpfsUploadFailed => "Failed to upload to IPFS",
            Self::IpfsTimeout => "IPFS operation timed out",
            Self::ExternalServiceError => "External service error",
        }
    }

    /// Get HTTP status code for this error
    pub fn http_status(&self) -> u16 {
        match self {
            // 1xxx - Client Errors -> 4xx
            Self::InvalidRequest
            | Self::InvalidImageId
            | Self::InvalidInputHash
            | Self::InvalidInputUrl
            | Self::InvalidRequestId
            | Self::InvalidProof
            | Self::InvalidSignature
            | Self::InputTooLarge
            | Self::InputTooSmall => 400,

            Self::RequestExpired => 410, // Gone
            Self::RequestNotFound => 404,
            Self::RequestAlreadyClaimed
            | Self::RequestNotClaimed
            | Self::RequestAlreadyCompleted => 409, // Conflict

            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::RateLimited | Self::QuotaExceeded => 429,

            // 2xxx - Server Errors -> 5xx
            Self::InternalError
            | Self::DatabaseError
            | Self::CacheError
            | Self::ConfigurationError => 500,

            Self::ServiceUnavailable
            | Self::ServiceOverloaded
            | Self::MaintenanceMode
            | Self::ShuttingDown => 503,

            // 3xxx - Proof Errors -> mix of 4xx/5xx
            Self::ProofGenerationFailed
            | Self::ProofVerificationFailed
            | Self::GuestProgramError
            | Self::GuestProgramPanic
            | Self::GuestProgramOOM => 500,

            Self::ProofTimeout | Self::GuestProgramTimeout | Self::BonsaiTimeout => 504, // Gateway Timeout
            Self::ProofCancelled => 499, // Client Closed Request

            Self::ImageNotFound => 404,
            Self::ImageNotRegistered | Self::ImageInactive => 400,

            Self::BonsaiError => 502,
            Self::BonsaiQuotaExceeded => 429,

            // 4xxx - Contract Errors -> 5xx
            Self::ContractError
            | Self::TransactionFailed
            | Self::TransactionReverted
            | Self::GasEstimationFailed
            | Self::NonceError => 500,

            Self::InsufficientFunds => 402, // Payment Required
            Self::ChainUnavailable | Self::ChainReorg => 503,
            Self::WrongChain => 500,

            // 5xxx - Network Errors -> 5xx
            Self::NetworkError
            | Self::ConnectionRefused
            | Self::DnsError
            | Self::TlsError
            | Self::IpfsError
            | Self::IpfsFetchFailed
            | Self::IpfsUploadFailed
            | Self::ExternalServiceError => 502,

            Self::ConnectionTimeout | Self::IpfsTimeout => 504,
        }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ServiceUnavailable
                | Self::ServiceOverloaded
                | Self::RateLimited
                | Self::ProofTimeout
                | Self::BonsaiTimeout
                | Self::ChainUnavailable
                | Self::NetworkError
                | Self::ConnectionTimeout
                | Self::ConnectionRefused
                | Self::IpfsTimeout
        )
    }

    /// Get suggested retry delay in milliseconds
    pub fn retry_delay_ms(&self) -> Option<u64> {
        if !self.is_retryable() {
            return None;
        }
        Some(match self {
            Self::RateLimited => 5000,
            Self::ServiceOverloaded => 10000,
            Self::ServiceUnavailable => 30000,
            Self::ProofTimeout | Self::BonsaiTimeout => 60000,
            _ => 1000,
        })
    }

    /// Parse from string code (e.g., "WZK-1000")
    pub fn from_code_string(s: &str) -> Option<Self> {
        let s = s.trim().to_uppercase();
        if !s.starts_with("WZK-") {
            return None;
        }
        let code: u16 = s[4..].parse().ok()?;
        Self::from_code(code)
    }

    /// Parse from numeric code
    pub fn from_code(code: u16) -> Option<Self> {
        Some(match code {
            1000 => Self::InvalidRequest,
            1001 => Self::InvalidImageId,
            1002 => Self::InvalidInputHash,
            1003 => Self::InvalidInputUrl,
            1004 => Self::InvalidRequestId,
            1005 => Self::InvalidProof,
            1006 => Self::InvalidSignature,
            1010 => Self::InputTooLarge,
            1011 => Self::InputTooSmall,
            1020 => Self::RequestExpired,
            1021 => Self::RequestNotFound,
            1022 => Self::RequestAlreadyClaimed,
            1023 => Self::RequestNotClaimed,
            1024 => Self::RequestAlreadyCompleted,
            1030 => Self::Unauthorized,
            1031 => Self::Forbidden,
            1040 => Self::RateLimited,
            1041 => Self::QuotaExceeded,

            2000 => Self::InternalError,
            2001 => Self::DatabaseError,
            2002 => Self::CacheError,
            2003 => Self::ConfigurationError,
            2010 => Self::ServiceUnavailable,
            2011 => Self::ServiceOverloaded,
            2012 => Self::MaintenanceMode,
            2013 => Self::ShuttingDown,

            3000 => Self::ProofGenerationFailed,
            3001 => Self::ProofVerificationFailed,
            3002 => Self::ProofTimeout,
            3003 => Self::ProofCancelled,
            3010 => Self::GuestProgramError,
            3011 => Self::GuestProgramPanic,
            3012 => Self::GuestProgramOOM,
            3013 => Self::GuestProgramTimeout,
            3020 => Self::ImageNotFound,
            3021 => Self::ImageNotRegistered,
            3022 => Self::ImageInactive,
            3030 => Self::BonsaiError,
            3031 => Self::BonsaiTimeout,
            3032 => Self::BonsaiQuotaExceeded,

            4000 => Self::ContractError,
            4001 => Self::TransactionFailed,
            4002 => Self::TransactionReverted,
            4003 => Self::InsufficientFunds,
            4004 => Self::GasEstimationFailed,
            4005 => Self::NonceError,
            4010 => Self::ChainUnavailable,
            4011 => Self::ChainReorg,
            4012 => Self::WrongChain,

            5000 => Self::NetworkError,
            5001 => Self::ConnectionTimeout,
            5002 => Self::ConnectionRefused,
            5003 => Self::DnsError,
            5004 => Self::TlsError,
            5010 => Self::IpfsError,
            5011 => Self::IpfsFetchFailed,
            5012 => Self::IpfsUploadFailed,
            5013 => Self::IpfsTimeout,
            5020 => Self::ExternalServiceError,

            _ => return None,
        })
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code_string())
    }
}

impl From<ErrorCode> for String {
    fn from(code: ErrorCode) -> String {
        code.code_string()
    }
}

impl TryFrom<String> for ErrorCode {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::from_code_string(&s).ok_or_else(|| format!("Unknown error code: {}", s))
    }
}

// ============================================================================
// API Error Response
// ============================================================================

/// Structured API error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    /// Error code (e.g., "WZK-1000")
    pub code: ErrorCode,

    /// Human-readable error message
    pub message: String,

    /// Additional details (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,

    /// Request ID for tracing (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// Trace ID for distributed tracing (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,

    /// Retry information (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryInfo>,
}

/// Retry information for retryable errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryInfo {
    /// Whether the request can be retried
    pub retryable: bool,

    /// Suggested delay before retry (milliseconds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,

    /// Maximum number of retries suggested
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
}

impl ApiError {
    /// Create a new API error with just a code
    pub fn new(code: ErrorCode) -> Self {
        Self {
            message: code.default_message().to_string(),
            code,
            details: None,
            request_id: None,
            trace_id: None,
            retry: if code.is_retryable() {
                Some(RetryInfo {
                    retryable: true,
                    retry_after_ms: code.retry_delay_ms(),
                    max_retries: Some(3),
                })
            } else {
                None
            },
        }
    }

    /// Create with custom message
    pub fn with_message(code: ErrorCode, message: impl Into<String>) -> Self {
        let mut err = Self::new(code);
        err.message = message.into();
        err
    }

    /// Add details
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Add request ID
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    /// Add trace ID
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// Get HTTP status code
    pub fn http_status(&self) -> u16 {
        self.code.http_status()
    }

    /// Convert to JSON string
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(r#"{{"code":"{}","message":"{}"}}"#, self.code, self.message)
        })
    }

    /// Convert to pretty JSON string
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| self.to_json())
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

// ============================================================================
// Convenience macros
// ============================================================================

/// Create an ApiError quickly
#[macro_export]
macro_rules! api_error {
    ($code:expr) => {
        $crate::errors::ApiError::new($code)
    };
    ($code:expr, $msg:expr) => {
        $crate::errors::ApiError::with_message($code, $msg)
    };
    ($code:expr, $msg:expr, $($key:tt: $value:expr),+ $(,)?) => {{
        let details = serde_json::json!({ $($key: $value),+ });
        $crate::errors::ApiError::with_message($code, $msg).with_details(details)
    }};
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_string() {
        assert_eq!(ErrorCode::InvalidRequest.code_string(), "WZK-1000");
        assert_eq!(ErrorCode::InternalError.code_string(), "WZK-2000");
        assert_eq!(ErrorCode::ProofGenerationFailed.code_string(), "WZK-3000");
        assert_eq!(ErrorCode::ContractError.code_string(), "WZK-4000");
        assert_eq!(ErrorCode::NetworkError.code_string(), "WZK-5000");
    }

    #[test]
    fn test_error_code_parse() {
        assert_eq!(
            ErrorCode::from_code_string("WZK-1000"),
            Some(ErrorCode::InvalidRequest)
        );
        assert_eq!(
            ErrorCode::from_code_string("wzk-2000"),
            Some(ErrorCode::InternalError)
        );
        assert_eq!(ErrorCode::from_code_string("INVALID"), None);
        assert_eq!(ErrorCode::from_code_string("WZK-9999"), None);
    }

    #[test]
    fn test_error_code_http_status() {
        assert_eq!(ErrorCode::InvalidRequest.http_status(), 400);
        assert_eq!(ErrorCode::Unauthorized.http_status(), 401);
        assert_eq!(ErrorCode::RequestNotFound.http_status(), 404);
        assert_eq!(ErrorCode::RateLimited.http_status(), 429);
        assert_eq!(ErrorCode::InternalError.http_status(), 500);
        assert_eq!(ErrorCode::ServiceUnavailable.http_status(), 503);
    }

    #[test]
    fn test_error_retryable() {
        assert!(ErrorCode::RateLimited.is_retryable());
        assert!(ErrorCode::ServiceUnavailable.is_retryable());
        assert!(ErrorCode::ConnectionTimeout.is_retryable());
        assert!(!ErrorCode::InvalidRequest.is_retryable());
        assert!(!ErrorCode::Unauthorized.is_retryable());
    }

    #[test]
    fn test_api_error_json() {
        let err = ApiError::new(ErrorCode::InvalidImageId);
        let json = err.to_json();
        assert!(json.contains("WZK-1001"));
        assert!(json.contains("Invalid image ID"));
    }

    #[test]
    fn test_api_error_with_details() {
        let err = ApiError::with_message(ErrorCode::InputTooLarge, "File too big")
            .with_details(serde_json::json!({
                "max_size": 10485760,
                "actual_size": 20000000
            }));

        let json = err.to_json();
        assert!(json.contains("max_size"));
        assert!(json.contains("10485760"));
    }

    #[test]
    fn test_api_error_macro() {
        let err = api_error!(ErrorCode::InvalidRequest);
        assert_eq!(err.code, ErrorCode::InvalidRequest);

        let err = api_error!(ErrorCode::InputTooLarge, "Too big");
        assert_eq!(err.message, "Too big");
    }

    #[test]
    fn test_error_code_serde() {
        let code = ErrorCode::InvalidRequest;
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, r#""WZK-1000""#);

        let parsed: ErrorCode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, code);
    }
}
