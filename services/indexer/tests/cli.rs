//! Command line of the `tee-indexer` binary.

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// How long a run may take, until both pipes are closed. Every case exits at
/// once; one that is still running after this has started the server or
/// hangs, and fails.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Run the binary in an empty directory, with an environment under which it
/// cannot start the server: an accepted command line ends in "invalid PORT".
/// Returns the output and the files the run left behind.
fn run<S: AsRef<OsStr>>(args: &[S]) -> (Output, usize) {
    let deadline = Instant::now() + TIMEOUT;
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tee-indexer"))
        .args(args)
        .env_clear()
        .env("PORT", "not-a-port")
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Read both pipes while waiting, so a full pipe cannot stall the child.
    let stdout = read_in_background(child.stdout.take().unwrap());
    let stderr = read_in_background(child.stderr.take().unwrap());
    let status = wait_or_kill(&mut child, deadline, args);
    // A process the child left behind can hold the pipes open after it has
    // exited, so the reads get the rest of the time and no more.
    let out = Output {
        status,
        stdout: recv_before(&stdout, deadline, "stdout", args),
        stderr: recv_before(&stderr, deadline, "stderr", args),
    };
    let files = std::fs::read_dir(dir.path()).unwrap().count();
    (out, files)
}

/// Read the pipe to its end on another thread, which sends what it read.
fn read_in_background(mut pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).unwrap();
        // The receiver is gone only after the test has failed.
        let _ = sender.send(bytes);
    });
    receiver
}

/// What the reader of a pipe read; fail if the pipe is still open at the
/// deadline or the reader failed.
fn recv_before<S: AsRef<OsStr>>(
    pipe: &Receiver<Vec<u8>>,
    deadline: Instant,
    name: &str,
    args: &[S],
) -> Vec<u8> {
    pipe.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_else(|e| {
            panic!(
                "tee-indexer {:?}: {name} not read to its end within {TIMEOUT:?}: {e}",
                shown(args)
            )
        })
}

/// Wait for the child to exit; kill it and fail if it runs past the deadline.
fn wait_or_kill<S: AsRef<OsStr>>(
    child: &mut Child,
    deadline: Instant,
    args: &[S],
) -> std::process::ExitStatus {
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "tee-indexer {:?} still running after {TIMEOUT:?}",
                shown(args)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn shown<S: AsRef<OsStr>>(args: &[S]) -> Vec<std::ffi::OsString> {
    args.iter().map(|a| a.as_ref().to_owned()).collect()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn help_prints_the_usage() {
    for arg in ["--help", "-h"] {
        let (out, files) = run(&[arg]);
        assert_eq!(out.status.code(), Some(0), "{arg}: {out:?}");
        assert_eq!(files, 0, "{arg}");
        assert!(out.stderr.is_empty(), "{arg}: {out:?}");

        let stdout = text(&out.stdout);
        assert!(stdout.starts_with("Usage: tee-indexer"), "{stdout}");
        for variable in [
            "RPC_URL",
            "CONTRACT_ADDRESS",
            "POLL_INTERVAL_SECS",
            "PORT",
            "DB_TYPE",
            "DB_PATH",
            "DATABASE_URL",
            "DB_MAX_CONNECTIONS",
            "DB_MIN_CONNECTIONS",
            "DB_ACQUIRE_TIMEOUT_SECS",
            "DB_MAX_LIFETIME_SECS",
            "DB_IDLE_TIMEOUT_SECS",
            "API_KEY",
            "ADMIN_API_KEY",
            "RATE_LIMIT_PER_IP",
            "TRUSTED_PROXIES",
            "MAX_WS_CONNECTIONS",
            "MAX_WS_CONNECTIONS_PER_IP",
            "LOG_FORMAT",
            "RUST_LOG",
        ] {
            assert!(
                stdout
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .any(|word| word == variable),
                "{variable} is not in the usage: {stdout}"
            );
        }
    }
}

#[test]
fn version_prints_name_and_version() {
    for arg in ["--version", "-V"] {
        let (out, files) = run(&[arg]);
        assert_eq!(out.status.code(), Some(0), "{arg}: {out:?}");
        assert_eq!(files, 0, "{arg}");
        assert_eq!(
            text(&out.stdout),
            concat!("tee-indexer ", env!("CARGO_PKG_VERSION"), "\n")
        );
    }
}

#[test]
fn other_arguments_are_an_error() {
    for args in [
        &["--bogus"][..],
        &["serve"],
        &[""],
        &["--port=1"],
        &["--bogus", "--help"],
        &["--help", "--bogus"],
        &["--"],
        &["--", "--help"],
    ] {
        let (out, files) = run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        assert_eq!(files, 0, "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}: {out:?}");
        assert!(
            text(&out.stderr).contains("unexpected argument"),
            "{args:?}: {out:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8_is_an_error() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    // The argument is shown with the bytes that are not UTF-8 replaced.
    for (bytes, shown) in [
        (&b"\x80"[..], "\u{FFFD}"),
        (b"--help\xff", "--help\u{FFFD}"),
    ] {
        let arg = OsString::from_vec(bytes.to_vec());
        let (out, files) = run(&[&arg]);
        assert_eq!(out.status.code(), Some(2), "{arg:?}: {out:?}");
        assert_eq!(files, 0, "{arg:?}");
        assert!(out.stdout.is_empty(), "{arg:?}: {out:?}");
        assert!(
            text(&out.stderr).contains(&format!("unexpected argument \"{shown}\"")),
            "{arg:?}: {out:?}"
        );
    }
}

#[test]
fn no_arguments_start_the_service() {
    let (out, _) = run::<&str>(&[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out.stderr).contains("invalid PORT"), "{out:?}");
}
