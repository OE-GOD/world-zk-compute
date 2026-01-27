//! Test guest program for E2E testing.
//!
//! This program performs a simple verifiable computation:
//! 1. Reads an input number
//! 2. Computes various operations (square, factorial for small numbers)
//! 3. Commits results to the journal for verification

#![no_main]

use risc0_zkvm::guest::env;
use serde::{Deserialize, Serialize};

risc0_zkvm::guest::entry!(main);

/// Input to the test program
#[derive(Debug, Serialize, Deserialize)]
pub struct TestInput {
    /// Number to compute on
    pub value: u64,
    /// Operation to perform
    pub operation: String,
}

/// Output from the test program
#[derive(Debug, Serialize, Deserialize)]
pub struct TestOutput {
    /// Original input value
    pub input: u64,
    /// Operation performed
    pub operation: String,
    /// Result of the computation
    pub result: u64,
    /// Verification hash (simple checksum)
    pub checksum: u64,
}

fn main() {
    // Read input from the host
    let input: TestInput = env::read();

    // Perform the requested operation
    let result = match input.operation.as_str() {
        "square" => input.value.saturating_mul(input.value),
        "double" => input.value.saturating_mul(2),
        "factorial" => factorial(input.value.min(20)), // Limit to prevent overflow
        "fibonacci" => fibonacci(input.value.min(90) as usize),
        "is_prime" => if is_prime(input.value) { 1 } else { 0 },
        "sum_to_n" => sum_to_n(input.value),
        _ => input.value, // Identity for unknown operations
    };

    // Create output with verification checksum
    let checksum = compute_checksum(input.value, &input.operation, result);

    let output = TestOutput {
        input: input.value,
        operation: input.operation,
        result,
        checksum,
    };

    // Commit the output to the journal (public output)
    env::commit(&output);
}

/// Compute factorial iteratively
fn factorial(n: u64) -> u64 {
    if n <= 1 {
        return 1;
    }
    let mut result = 1u64;
    for i in 2..=n {
        result = result.saturating_mul(i);
    }
    result
}

/// Compute fibonacci iteratively
fn fibonacci(n: usize) -> u64 {
    if n <= 1 {
        return n as u64;
    }
    let mut a = 0u64;
    let mut b = 1u64;
    for _ in 2..=n {
        let c = a.saturating_add(b);
        a = b;
        b = c;
    }
    b
}

/// Simple primality check
fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    if n == 2 {
        return true;
    }
    if n % 2 == 0 {
        return false;
    }
    let mut i = 3u64;
    while i * i <= n {
        if n % i == 0 {
            return false;
        }
        i += 2;
    }
    true
}

/// Sum from 1 to n
fn sum_to_n(n: u64) -> u64 {
    // Using formula: n * (n + 1) / 2
    n.saturating_mul(n.saturating_add(1)) / 2
}

/// Compute a simple checksum for verification
fn compute_checksum(input: u64, operation: &str, result: u64) -> u64 {
    let op_hash: u64 = operation.bytes().fold(0u64, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(b as u64)
    });
    input
        .wrapping_mul(17)
        .wrapping_add(op_hash)
        .wrapping_mul(13)
        .wrapping_add(result)
}
