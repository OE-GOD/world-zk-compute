//! Library re-exports for TEE enclave modules.
//!
//! The enclave binary is in `main.rs`. This file exposes the testable
//! modules so that integration and chaos tests can import them directly
//! without running the full server.

pub mod metrics;
pub mod model_registry;
pub mod validation;
pub mod watchdog;
