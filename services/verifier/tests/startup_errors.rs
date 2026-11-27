//! The service binary stops with exit code 1 and logs the reason when its
//! settings are wrong, instead of starting with defaults.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Long enough for a debug build on a busy machine; a service that ignored
/// the bad setting would keep running and be killed at this point.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Run the service with only `vars` set and wait for it to exit.
fn run_service(vars: &[(&str, &str)]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_zkml-verifier-service"))
        .env_clear()
        .envs(vars.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the service");

    let deadline = Instant::now() + TIMEOUT;
    while child.try_wait().expect("wait for the service").is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the service with {vars:?} did not exit within {TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The few log lines fit in the pipe buffers, so the service could not
    // block on a full pipe before exiting.
    child.wait_with_output().expect("read the service output")
}

/// Check that the service exited with code 1 and logged `message`.
fn assert_stops_with(vars: &[(&str, &str)], message: &str) {
    let output = run_service(vars);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{vars:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
    // tracing writes the log to stdout.
    assert!(
        stdout.contains(message),
        "{vars:?}: no {message:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn port_that_is_not_a_number_stops_start_up() {
    assert_stops_with(
        &[("PORT", "not-a-number")],
        "Configuration error: PORT has an invalid value",
    );
}

#[test]
fn tls_key_without_a_certificate_stops_start_up() {
    // PORT=0: if the check were missing, the service would listen on a free
    // port until the timeout rather than take a fixed one.
    assert_stops_with(
        &[("PORT", "0"), ("VERIFIER_TLS_KEY", "/nonexistent/key.pem")],
        "TLS configuration error: VERIFIER_TLS_KEY is set but VERIFIER_TLS_CERT is missing",
    );
}
