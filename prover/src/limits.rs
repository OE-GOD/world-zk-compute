//! Resource Limits
//!
//! Enforces limits on inputs and resources to prevent:
//! - Memory exhaustion from large inputs
//! - CPU exhaustion from complex programs
//! - Disk exhaustion from large proofs
//! - DoS attacks via resource abuse
//!
//! ## Limits Hierarchy
//!
//! 1. Hard limits - Never exceeded, enforced at entry points
//! 2. Soft limits - Warnings issued, may be configurable
//! 3. Dynamic limits - Adjusted based on system resources

use std::time::Duration;
use thiserror::Error;
use tracing::{debug, warn};

/// Limit violation errors
#[derive(Debug, Error)]
pub enum LimitError {
    #[error("Input size {size} exceeds limit {limit} bytes")]
    InputTooLarge { size: usize, limit: usize },

    #[error("Program size {size} exceeds limit {limit} bytes")]
    ProgramTooLarge { size: usize, limit: usize },

    #[error("Proof size {size} exceeds limit {limit} bytes")]
    ProofTooLarge { size: usize, limit: usize },

    #[error("Journal size {size} exceeds limit {limit} bytes")]
    JournalTooLarge { size: usize, limit: usize },

    #[error("URL length {length} exceeds limit {limit}")]
    UrlTooLong { length: usize, limit: usize },

    #[error("Execution time {elapsed:?} exceeds limit {limit:?}")]
    ExecutionTimeout { elapsed: Duration, limit: Duration },

    #[error("Memory usage {used} exceeds limit {limit} bytes")]
    MemoryExceeded { used: usize, limit: usize },

    #[error("Segment count {count} exceeds limit {limit}")]
    TooManySegments { count: usize, limit: usize },

    #[error("Queue size {size} exceeds limit {limit}")]
    QueueFull { size: usize, limit: usize },

    #[error("Concurrent jobs {count} exceeds limit {limit}")]
    TooManyConcurrent { count: usize, limit: usize },

    #[error("Rate limit exceeded: {message}")]
    RateLimitExceeded { message: String },
}

/// Resource limits configuration
#[derive(Debug, Clone)]
pub struct Limits {
    // === Input Limits ===
    /// Maximum input data size
    pub max_input_size: usize,
    /// Maximum URL length
    pub max_url_length: usize,
    /// Maximum number of inputs per request
    pub max_inputs_per_request: usize,

    // === Program Limits ===
    /// Maximum ELF binary size
    pub max_program_size: usize,
    /// Maximum segments for continuations
    pub max_segments: usize,
    /// Maximum cycles per segment
    pub max_cycles_per_segment: u64,

    // === Output Limits ===
    /// Maximum proof size
    pub max_proof_size: usize,
    /// Maximum journal size
    pub max_journal_size: usize,

    // === Execution Limits ===
    /// Maximum proof generation time
    pub max_proof_time: Duration,
    /// Maximum input fetch time
    pub max_fetch_time: Duration,
    /// Maximum program load time
    pub max_load_time: Duration,

    // === Memory Limits ===
    /// Maximum memory for proof generation
    pub max_prover_memory: usize,
    /// Maximum cache size
    pub max_cache_size: usize,

    // === Concurrency Limits ===
    /// Maximum concurrent proof jobs
    pub max_concurrent_proofs: usize,
    /// Maximum pending jobs in queue
    pub max_queue_size: usize,
    /// Maximum active network connections
    pub max_connections: usize,

    // === Rate Limits ===
    /// Maximum claims per minute
    pub max_claims_per_minute: u32,
    /// Maximum RPC calls per second
    pub max_rpc_per_second: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Input limits
            max_input_size: 100 * 1024 * 1024,      // 100 MB
            max_url_length: 2048,                   // 2 KB
            max_inputs_per_request: 100,

            // Program limits
            max_program_size: 256 * 1024 * 1024,    // 256 MB
            max_segments: 1000,
            max_cycles_per_segment: 1 << 24,        // ~16M cycles

            // Output limits
            max_proof_size: 500 * 1024 * 1024,      // 500 MB (STARK proofs are large)
            max_journal_size: 10 * 1024 * 1024,     // 10 MB

            // Execution limits
            max_proof_time: Duration::from_secs(3600),     // 1 hour
            max_fetch_time: Duration::from_secs(60),       // 1 minute
            max_load_time: Duration::from_secs(30),        // 30 seconds

            // Memory limits
            max_prover_memory: 32 * 1024 * 1024 * 1024,    // 32 GB
            max_cache_size: 10 * 1024 * 1024 * 1024,       // 10 GB

            // Concurrency limits
            max_concurrent_proofs: 8,
            max_queue_size: 1000,
            max_connections: 100,

            // Rate limits
            max_claims_per_minute: 60,
            max_rpc_per_second: 100,
        }
    }
}

impl Limits {
    /// Create limits for a resource-constrained environment
    pub fn constrained() -> Self {
        Self {
            max_input_size: 10 * 1024 * 1024,       // 10 MB
            max_program_size: 64 * 1024 * 1024,     // 64 MB
            max_proof_size: 100 * 1024 * 1024,      // 100 MB
            max_prover_memory: 8 * 1024 * 1024 * 1024,  // 8 GB
            max_concurrent_proofs: 2,
            max_queue_size: 100,
            ..Default::default()
        }
    }

    /// Create limits for a high-performance environment
    pub fn high_performance() -> Self {
        Self {
            max_input_size: 500 * 1024 * 1024,      // 500 MB
            max_program_size: 1024 * 1024 * 1024,   // 1 GB
            max_proof_size: 2 * 1024 * 1024 * 1024, // 2 GB
            max_prover_memory: 128 * 1024 * 1024 * 1024, // 128 GB
            max_concurrent_proofs: 32,
            max_queue_size: 10000,
            max_proof_time: Duration::from_secs(7200), // 2 hours
            ..Default::default()
        }
    }

    /// Create limits from environment variables
    pub fn from_env() -> Self {
        let mut limits = Self::default();

        if let Ok(val) = std::env::var("MAX_INPUT_SIZE_MB") {
            if let Ok(mb) = val.parse::<usize>() {
                limits.max_input_size = mb * 1024 * 1024;
            }
        }

        if let Ok(val) = std::env::var("MAX_CONCURRENT_PROOFS") {
            if let Ok(n) = val.parse() {
                limits.max_concurrent_proofs = n;
            }
        }

        if let Ok(val) = std::env::var("MAX_PROOF_TIME_SECS") {
            if let Ok(secs) = val.parse() {
                limits.max_proof_time = Duration::from_secs(secs);
            }
        }

        if let Ok(val) = std::env::var("MAX_QUEUE_SIZE") {
            if let Ok(n) = val.parse() {
                limits.max_queue_size = n;
            }
        }

        limits
    }
}

/// Limit checker with tracking
pub struct LimitChecker {
    limits: Limits,
    /// Soft limit percentage (warn when exceeded)
    soft_limit_percent: f64,
}

impl LimitChecker {
    /// Create a new limit checker
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            soft_limit_percent: 0.8, // Warn at 80%
        }
    }

    /// Check input size limit
    pub fn check_input_size(&self, size: usize) -> Result<(), LimitError> {
        self.warn_if_close("input_size", size, self.limits.max_input_size);

        if size > self.limits.max_input_size {
            return Err(LimitError::InputTooLarge {
                size,
                limit: self.limits.max_input_size,
            });
        }
        Ok(())
    }

    /// Check program size limit
    pub fn check_program_size(&self, size: usize) -> Result<(), LimitError> {
        self.warn_if_close("program_size", size, self.limits.max_program_size);

        if size > self.limits.max_program_size {
            return Err(LimitError::ProgramTooLarge {
                size,
                limit: self.limits.max_program_size,
            });
        }
        Ok(())
    }

    /// Check proof size limit
    pub fn check_proof_size(&self, size: usize) -> Result<(), LimitError> {
        self.warn_if_close("proof_size", size, self.limits.max_proof_size);

        if size > self.limits.max_proof_size {
            return Err(LimitError::ProofTooLarge {
                size,
                limit: self.limits.max_proof_size,
            });
        }
        Ok(())
    }

    /// Check journal size limit
    pub fn check_journal_size(&self, size: usize) -> Result<(), LimitError> {
        self.warn_if_close("journal_size", size, self.limits.max_journal_size);

        if size > self.limits.max_journal_size {
            return Err(LimitError::JournalTooLarge {
                size,
                limit: self.limits.max_journal_size,
            });
        }
        Ok(())
    }

    /// Check URL length limit
    pub fn check_url_length(&self, length: usize) -> Result<(), LimitError> {
        if length > self.limits.max_url_length {
            return Err(LimitError::UrlTooLong {
                length,
                limit: self.limits.max_url_length,
            });
        }
        Ok(())
    }

    /// Check segment count limit
    pub fn check_segment_count(&self, count: usize) -> Result<(), LimitError> {
        self.warn_if_close("segments", count, self.limits.max_segments);

        if count > self.limits.max_segments {
            return Err(LimitError::TooManySegments {
                count,
                limit: self.limits.max_segments,
            });
        }
        Ok(())
    }

    /// Check queue size limit
    pub fn check_queue_size(&self, size: usize) -> Result<(), LimitError> {
        self.warn_if_close("queue_size", size, self.limits.max_queue_size);

        if size > self.limits.max_queue_size {
            return Err(LimitError::QueueFull {
                size,
                limit: self.limits.max_queue_size,
            });
        }
        Ok(())
    }

    /// Check concurrent jobs limit
    pub fn check_concurrent(&self, count: usize) -> Result<(), LimitError> {
        if count > self.limits.max_concurrent_proofs {
            return Err(LimitError::TooManyConcurrent {
                count,
                limit: self.limits.max_concurrent_proofs,
            });
        }
        Ok(())
    }

    /// Check execution time
    pub fn check_execution_time(&self, elapsed: Duration) -> Result<(), LimitError> {
        if elapsed > self.limits.max_proof_time {
            return Err(LimitError::ExecutionTimeout {
                elapsed,
                limit: self.limits.max_proof_time,
            });
        }
        Ok(())
    }

    /// Check memory usage
    pub fn check_memory(&self, used: usize) -> Result<(), LimitError> {
        self.warn_if_close("memory", used, self.limits.max_prover_memory);

        if used > self.limits.max_prover_memory {
            return Err(LimitError::MemoryExceeded {
                used,
                limit: self.limits.max_prover_memory,
            });
        }
        Ok(())
    }

    /// Warn if approaching limit
    fn warn_if_close(&self, name: &str, current: usize, limit: usize) {
        let threshold = (limit as f64 * self.soft_limit_percent) as usize;
        if current > threshold {
            let percent = (current as f64 / limit as f64) * 100.0;
            warn!(
                "{} at {:.1}% of limit ({} / {})",
                name, percent, current, limit
            );
        }
    }

    /// Get the underlying limits
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
}

/// Convenience function to check multiple limits at once
pub struct LimitCheckBuilder<'a> {
    checker: &'a LimitChecker,
    errors: Vec<LimitError>,
}

impl<'a> LimitCheckBuilder<'a> {
    pub fn new(checker: &'a LimitChecker) -> Self {
        Self {
            checker,
            errors: Vec::new(),
        }
    }

    pub fn input_size(mut self, size: usize) -> Self {
        if let Err(e) = self.checker.check_input_size(size) {
            self.errors.push(e);
        }
        self
    }

    pub fn program_size(mut self, size: usize) -> Self {
        if let Err(e) = self.checker.check_program_size(size) {
            self.errors.push(e);
        }
        self
    }

    pub fn url_length(mut self, length: usize) -> Self {
        if let Err(e) = self.checker.check_url_length(length) {
            self.errors.push(e);
        }
        self
    }

    pub fn queue_size(mut self, size: usize) -> Self {
        if let Err(e) = self.checker.check_queue_size(size) {
            self.errors.push(e);
        }
        self
    }

    /// Finish and return any errors
    pub fn finish(self) -> Result<(), Vec<LimitError>> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors)
        }
    }

    /// Finish and return first error if any
    pub fn finish_first(self) -> Result<(), LimitError> {
        if let Some(err) = self.errors.into_iter().next() {
            Err(err)
        } else {
            Ok(())
        }
    }
}

/// Global limit checker instance
static LIMITS: once_cell::sync::OnceCell<LimitChecker> = once_cell::sync::OnceCell::new();

/// Initialize global limits
pub fn init_limits(limits: Limits) -> &'static LimitChecker {
    LIMITS.get_or_init(|| LimitChecker::new(limits))
}

/// Get global limit checker
pub fn limits() -> &'static LimitChecker {
    LIMITS.get_or_init(|| LimitChecker::new(Limits::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_size_limit() {
        let checker = LimitChecker::new(Limits {
            max_input_size: 1000,
            ..Default::default()
        });

        assert!(checker.check_input_size(500).is_ok());
        assert!(checker.check_input_size(1000).is_ok());
        assert!(checker.check_input_size(1001).is_err());
    }

    #[test]
    fn test_program_size_limit() {
        let checker = LimitChecker::new(Limits {
            max_program_size: 1024,
            ..Default::default()
        });

        assert!(checker.check_program_size(1024).is_ok());
        assert!(checker.check_program_size(2048).is_err());
    }

    #[test]
    fn test_url_length_limit() {
        let checker = LimitChecker::new(Limits {
            max_url_length: 100,
            ..Default::default()
        });

        assert!(checker.check_url_length(50).is_ok());
        assert!(checker.check_url_length(101).is_err());
    }

    #[test]
    fn test_execution_time_limit() {
        let checker = LimitChecker::new(Limits {
            max_proof_time: Duration::from_secs(10),
            ..Default::default()
        });

        assert!(checker.check_execution_time(Duration::from_secs(5)).is_ok());
        assert!(checker.check_execution_time(Duration::from_secs(15)).is_err());
    }

    #[test]
    fn test_queue_size_limit() {
        let checker = LimitChecker::new(Limits {
            max_queue_size: 10,
            ..Default::default()
        });

        assert!(checker.check_queue_size(5).is_ok());
        assert!(checker.check_queue_size(11).is_err());
    }

    #[test]
    fn test_limit_check_builder() {
        let checker = LimitChecker::new(Limits {
            max_input_size: 1000,
            max_url_length: 100,
            ..Default::default()
        });

        // All pass
        let result = LimitCheckBuilder::new(&checker)
            .input_size(500)
            .url_length(50)
            .finish();
        assert!(result.is_ok());

        // One fails
        let result = LimitCheckBuilder::new(&checker)
            .input_size(2000)
            .url_length(50)
            .finish_first();
        assert!(result.is_err());
    }

    #[test]
    fn test_constrained_limits() {
        let limits = Limits::constrained();
        assert!(limits.max_input_size < Limits::default().max_input_size);
        assert!(limits.max_concurrent_proofs < Limits::default().max_concurrent_proofs);
    }

    #[test]
    fn test_high_performance_limits() {
        let limits = Limits::high_performance();
        assert!(limits.max_input_size > Limits::default().max_input_size);
        assert!(limits.max_concurrent_proofs > Limits::default().max_concurrent_proofs);
    }

    #[test]
    fn test_limit_errors() {
        let err = LimitError::InputTooLarge {
            size: 100,
            limit: 50,
        };
        assert!(err.to_string().contains("100"));
        assert!(err.to_string().contains("50"));
    }
}
