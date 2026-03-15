//! Typed error types for the World ZK Compute SDK.
//!
//! Provides [`SDKError`] — a structured error enum that lets callers match on
//! specific failure categories (network, contract, validation, timeout,
//! authentication, not-found) and apply targeted retry or fallback logic.
//!
//! The existing public API continues to return `anyhow::Result`. These types
//! are additive: adopt them incrementally without breaking existing callers.
//!
//! # Example
//!
//! ```rust
//! use world_zk_sdk::error::{SDKError, SDKResult};
//!
//! fn validate_input(data: &[u8]) -> SDKResult<()> {
//!     if data.is_empty() {
//!         return Err(SDKError::Validation("input must not be empty".into()));
//!     }
//!     Ok(())
//! }
//!
//! fn handle(err: SDKError) {
//!     match err {
//!         SDKError::Network(msg) => eprintln!("network error, will retry: {msg}"),
//!         SDKError::Contract(msg) => eprintln!("contract reverted: {msg}"),
//!         SDKError::Validation(msg) => eprintln!("bad input: {msg}"),
//!         SDKError::Timeout(msg) => eprintln!("timed out: {msg}"),
//!         SDKError::Auth(msg) => eprintln!("auth failure: {msg}"),
//!         SDKError::NotFound(msg) => eprintln!("not found: {msg}"),
//!         SDKError::Other(e) => eprintln!("unexpected: {e}"),
//!     }
//! }
//! ```

use std::fmt;

/// Structured error type for the World ZK Compute SDK.
///
/// Each variant captures a human-readable message describing the failure.
/// `Other` wraps an opaque [`anyhow::Error`] for errors that do not fit a
/// known category.
#[derive(Debug)]
pub enum SDKError {
    /// A transport-level or RPC communication failure (connection refused,
    /// DNS resolution, HTTP 5xx, rate-limiting, etc.).
    Network(String),

    /// The on-chain contract call reverted or returned an unexpected result.
    Contract(String),

    /// A caller-supplied argument failed a precondition check (empty proof,
    /// bad address format, etc.).
    Validation(String),

    /// An operation exceeded its deadline (per-attempt or total timeout).
    Timeout(String),

    /// An authentication or authorization failure (wrong signer, missing
    /// permissions, invalid private key).
    Auth(String),

    /// A requested resource was not found (unknown circuit hash, missing
    /// session, etc.).
    NotFound(String),

    /// A catch-all for errors that do not map to a specific variant.
    Other(anyhow::Error),
}

impl fmt::Display for SDKError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SDKError::Network(msg) => write!(f, "network error: {msg}"),
            SDKError::Contract(msg) => write!(f, "contract error: {msg}"),
            SDKError::Validation(msg) => write!(f, "validation error: {msg}"),
            SDKError::Timeout(msg) => write!(f, "timeout: {msg}"),
            SDKError::Auth(msg) => write!(f, "auth error: {msg}"),
            SDKError::NotFound(msg) => write!(f, "not found: {msg}"),
            SDKError::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SDKError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SDKError::Other(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for SDKError {
    fn from(err: anyhow::Error) -> Self {
        SDKError::Other(err)
    }
}

/// Convenience alias for `Result<T, SDKError>`.
pub type SDKResult<T> = Result<T, SDKError>;

// ---------------------------------------------------------------------------
// Helper constructors
// ---------------------------------------------------------------------------

impl SDKError {
    /// Create a [`Network`](SDKError::Network) error from anything that
    /// implements `Display`.
    pub fn network(msg: impl fmt::Display) -> Self {
        SDKError::Network(msg.to_string())
    }

    /// Create a [`Contract`](SDKError::Contract) error.
    pub fn contract(msg: impl fmt::Display) -> Self {
        SDKError::Contract(msg.to_string())
    }

    /// Create a [`Validation`](SDKError::Validation) error.
    pub fn validation(msg: impl fmt::Display) -> Self {
        SDKError::Validation(msg.to_string())
    }

    /// Create a [`Timeout`](SDKError::Timeout) error.
    pub fn timeout(msg: impl fmt::Display) -> Self {
        SDKError::Timeout(msg.to_string())
    }

    /// Create an [`Auth`](SDKError::Auth) error.
    pub fn auth(msg: impl fmt::Display) -> Self {
        SDKError::Auth(msg.to_string())
    }

    /// Create a [`NotFound`](SDKError::NotFound) error.
    pub fn not_found(msg: impl fmt::Display) -> Self {
        SDKError::NotFound(msg.to_string())
    }

    /// Returns `true` if this error is likely transient and the operation
    /// could succeed on retry (network errors, timeouts).
    pub fn is_retryable(&self) -> bool {
        matches!(self, SDKError::Network(_) | SDKError::Timeout(_))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_network() {
        let e = SDKError::Network("connection refused".into());
        assert_eq!(e.to_string(), "network error: connection refused");
    }

    #[test]
    fn display_contract() {
        let e = SDKError::Contract("execution reverted".into());
        assert_eq!(e.to_string(), "contract error: execution reverted");
    }

    #[test]
    fn display_validation() {
        let e = SDKError::Validation("proof is empty".into());
        assert_eq!(e.to_string(), "validation error: proof is empty");
    }

    #[test]
    fn display_timeout() {
        let e = SDKError::Timeout("exceeded 30s".into());
        assert_eq!(e.to_string(), "timeout: exceeded 30s");
    }

    #[test]
    fn display_auth() {
        let e = SDKError::Auth("invalid private key".into());
        assert_eq!(e.to_string(), "auth error: invalid private key");
    }

    #[test]
    fn display_not_found() {
        let e = SDKError::NotFound("circuit 0xabc not registered".into());
        assert_eq!(e.to_string(), "not found: circuit 0xabc not registered");
    }

    #[test]
    fn display_other() {
        let e = SDKError::Other(anyhow::anyhow!("something unexpected"));
        assert_eq!(e.to_string(), "something unexpected");
    }

    #[test]
    fn from_anyhow() {
        let anyhow_err = anyhow::anyhow!("low-level failure");
        let sdk_err: SDKError = anyhow_err.into();
        assert!(matches!(sdk_err, SDKError::Other(_)));
        assert_eq!(sdk_err.to_string(), "low-level failure");
    }

    #[test]
    fn is_retryable_network() {
        assert!(SDKError::Network("timeout".into()).is_retryable());
    }

    #[test]
    fn is_retryable_timeout() {
        assert!(SDKError::Timeout("deadline".into()).is_retryable());
    }

    #[test]
    fn is_not_retryable_validation() {
        assert!(!SDKError::Validation("bad".into()).is_retryable());
    }

    #[test]
    fn is_not_retryable_contract() {
        assert!(!SDKError::Contract("reverted".into()).is_retryable());
    }

    #[test]
    fn is_not_retryable_auth() {
        assert!(!SDKError::Auth("denied".into()).is_retryable());
    }

    #[test]
    fn is_not_retryable_not_found() {
        assert!(!SDKError::NotFound("missing".into()).is_retryable());
    }

    #[test]
    fn is_not_retryable_other() {
        assert!(!SDKError::Other(anyhow::anyhow!("??")).is_retryable());
    }

    #[test]
    fn helper_constructors() {
        let e = SDKError::network("conn reset");
        assert!(matches!(e, SDKError::Network(ref m) if m == "conn reset"));

        let e = SDKError::contract("revert");
        assert!(matches!(e, SDKError::Contract(ref m) if m == "revert"));

        let e = SDKError::validation("empty");
        assert!(matches!(e, SDKError::Validation(ref m) if m == "empty"));

        let e = SDKError::timeout("5s");
        assert!(matches!(e, SDKError::Timeout(ref m) if m == "5s"));

        let e = SDKError::auth("bad key");
        assert!(matches!(e, SDKError::Auth(ref m) if m == "bad key"));

        let e = SDKError::not_found("gone");
        assert!(matches!(e, SDKError::NotFound(ref m) if m == "gone"));
    }

    #[test]
    fn error_source_for_other() {
        let inner = anyhow::anyhow!("root cause");
        let e = SDKError::Other(inner);
        // std::error::Error::source() should return Some for Other
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn error_source_for_variant() {
        let e = SDKError::Network("test".into());
        assert!(std::error::Error::source(&e).is_none());
    }

    #[test]
    fn sdk_result_alias() {
        fn example() -> SDKResult<u64> {
            Ok(42)
        }
        assert_eq!(example().unwrap(), 42);
    }

    #[test]
    fn sdk_result_alias_err() {
        fn example() -> SDKResult<u64> {
            Err(SDKError::validation("bad"))
        }
        assert!(example().is_err());
    }
}
