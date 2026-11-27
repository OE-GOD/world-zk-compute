//! A bad S3 setting stops the service with exit code 1 before it contacts S3.
//! With the `s3` feature that is a bad `S3_RETENTION_YEARS`, an empty
//! `S3_BUCKET` or a value that is not valid UTF-8; without it, any non-empty
//! `S3_BUCKET`, since the build cannot use it.

use std::ffi::OsStr;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Long enough for a debug build on a busy machine; a service that ignored
/// the bad setting would go on to start and be killed at this point.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Start the service with only `vars` set, in `dir`. PORT=0: a service that
/// starts listens on a free port rather than take a fixed one.
fn spawn_service(dir: &std::path::Path, vars: &[(&str, &OsStr)]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_proof-registry"))
        .env_clear()
        .env("PORT", "0")
        .envs(vars.iter().copied())
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the service")
}

/// Run the service with only `vars` set, in an empty directory, and wait for
/// it to exit.
fn run_service(vars: &[(&str, &OsStr)]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn_service(dir.path(), vars);

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

/// Assert that the service with `vars` exits with code 1 and logs `message`.
fn assert_start_up_fails(vars: &[(&str, &OsStr)], message: &str) {
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
        "{vars:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[cfg(feature = "s3")]
#[test]
fn bad_retention_years_stops_start_up() {
    for years in ["0", "10y"] {
        assert_start_up_fails(
            &[
                ("S3_BUCKET", OsStr::new("test-bucket")),
                ("S3_RETENTION_YEARS", OsStr::new(years)),
            ],
            "Failed to initialize S3 storage: S3_RETENTION_YEARS has an invalid value",
        );
    }
}

#[cfg(all(feature = "s3", unix))]
#[test]
fn non_utf8_s3_setting_stops_start_up() {
    use std::os::unix::ffi::OsStrExt;
    let bad = OsStr::from_bytes(b"value\xff");
    for name in ["S3_BUCKET", "S3_REGION", "S3_RETENTION_MODE", "S3_PREFIX"] {
        // The others valid, so only `name` can be the cause.
        let mut vars = vec![
            ("S3_BUCKET", OsStr::new("test-bucket")),
            ("S3_REGION", OsStr::new("us-east-1")),
            ("S3_RETENTION_MODE", OsStr::new("COMPLIANCE")),
            ("S3_PREFIX", OsStr::new("proofs/")),
        ];
        for var in &mut vars {
            if var.0 == name {
                var.1 = bad;
            }
        }
        assert_start_up_fails(
            &vars,
            &format!("Failed to initialize S3 storage: {name} has an invalid value"),
        );
    }
}

#[cfg(feature = "s3")]
#[test]
fn empty_s3_bucket_stops_start_up() {
    assert_start_up_fails(
        &[("S3_BUCKET", OsStr::new(""))],
        "Failed to initialize S3 storage: S3_BUCKET is set but empty",
    );
}

#[cfg(not(feature = "s3"))]
#[test]
fn s3_bucket_without_the_s3_feature_stops_start_up() {
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;
    let buckets = [
        OsStr::new("test-bucket"),
        #[cfg(unix)]
        OsStr::from_bytes(b"bucket\xff"),
    ];
    for bucket in buckets {
        assert_start_up_fails(
            &[("S3_BUCKET", bucket)],
            "S3_BUCKET is set, but this build has no S3 support (built without the s3 feature)",
        );
    }
}

#[cfg(not(feature = "s3"))]
#[test]
fn empty_s3_bucket_without_the_s3_feature_uses_local_storage() {
    use std::io::{BufRead, BufReader};

    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn_service(dir.path(), &[("S3_BUCKET", OsStr::new(""))]);

    // Read the log on another thread so that the wait can time out.
    let stdout = child.stdout.take().unwrap();
    let (lines, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + TIMEOUT;
    let mut log = String::new();
    // Ends when the service is listening, exits (the channel closes) or times out.
    while let Ok(line) = received.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        log.push_str(&line);
        log.push('\n');
        if line.contains("Proof registry listening") {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    assert!(log.contains("Storage backend: local"), "log: {log}");
    assert!(log.contains("Proof registry listening"), "log: {log}");
}
