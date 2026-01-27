//! Main E2E test entrypoint
//!
//! Run with: cargo test -p e2e-tests

use e2e_tests::{E2ERunner, TestCase};

#[tokio::test]
async fn test_e2e_square() {
    let runner = E2ERunner::default_config();
    let test = TestCase::square(5);
    let result = runner.run_test(&test).await;

    assert!(result.passed, "Square test failed: {:?}", result.error);
    assert_eq!(result.actual_result, Some(25));
}

#[tokio::test]
async fn test_e2e_factorial() {
    let runner = E2ERunner::default_config();
    let test = TestCase::factorial(5);
    let result = runner.run_test(&test).await;

    assert!(result.passed, "Factorial test failed: {:?}", result.error);
    assert_eq!(result.actual_result, Some(120));
}

#[tokio::test]
async fn test_e2e_fibonacci() {
    let runner = E2ERunner::default_config();
    let test = TestCase::fibonacci(10);
    let result = runner.run_test(&test).await;

    assert!(result.passed, "Fibonacci test failed: {:?}", result.error);
    assert_eq!(result.actual_result, Some(55));
}

#[tokio::test]
async fn test_e2e_prime() {
    let runner = E2ERunner::default_config();

    // Test prime number
    let test = TestCase::is_prime(17, true);
    let result = runner.run_test(&test).await;
    assert!(result.passed, "Prime test failed for 17: {:?}", result.error);
    assert_eq!(result.actual_result, Some(1));

    // Test non-prime number
    let test = TestCase::is_prime(4, false);
    let result = runner.run_test(&test).await;
    assert!(result.passed, "Prime test failed for 4: {:?}", result.error);
    assert_eq!(result.actual_result, Some(0));
}

#[tokio::test]
async fn test_e2e_standard_suite() {
    let runner = E2ERunner::default_config();
    let results = runner.run_standard_suite().await;

    results.print_summary();

    assert!(
        results.all_passed(),
        "E2E test suite failed: {} of {} tests failed",
        results.failed,
        results.total
    );
}

#[tokio::test]
async fn test_e2e_custom_tests() {
    let runner = E2ERunner::default_config();

    // Custom test cases
    let custom_tests = vec![
        TestCase::square(0),      // Edge case: zero
        TestCase::square(1),      // Edge case: one
        TestCase::factorial(0),   // Edge case: 0! = 1
        TestCase::factorial(1),   // Edge case: 1! = 1
        TestCase::fibonacci(0),   // Edge case: fib(0) = 0
        TestCase::fibonacci(1),   // Edge case: fib(1) = 1
        TestCase::is_prime(2, true),   // Smallest prime
        TestCase::is_prime(1, false),  // Not prime
    ];

    let results = runner.run_all(&custom_tests).await;

    assert!(
        results.all_passed(),
        "Custom E2E tests failed: {} of {} tests failed",
        results.failed,
        results.total
    );
}

#[tokio::test]
async fn test_e2e_large_numbers() {
    let runner = E2ERunner::default_config();

    // Test with larger numbers
    let large_tests = vec![
        TestCase::square(12345),
        TestCase::factorial(20),   // Largest factorial before overflow
        TestCase::fibonacci(50),   // Large fibonacci
        TestCase::is_prime(97, true),  // Larger prime
        TestCase::is_prime(100, false), // Non-prime
    ];

    let results = runner.run_all(&large_tests).await;

    assert!(
        results.all_passed(),
        "Large number E2E tests failed: {} of {} tests failed",
        results.failed,
        results.total
    );
}
