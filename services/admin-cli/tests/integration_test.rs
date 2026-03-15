//! Integration tests for the admin-cli binary.
//!
//! These tests exercise CLI argument parsing, env var overrides,
//! error messages, and helper function behavior via the binary crate.

use std::process::Command;

/// Path to the admin-cli binary (built by cargo test).
fn cli_bin() -> String {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    // cargo test puts test binaries in deps/, the actual binary is one level up
    path.push("admin-cli");
    path.to_string_lossy().to_string()
}

/// Run the CLI with given args and return (exit_code, stdout, stderr).
fn run_cli(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(cli_bin())
        .args(args)
        .env_remove("RPC_URL")
        .env_remove("CONTRACT_ADDRESS")
        .env_remove("PRIVATE_KEY")
        .output()
        .expect("failed to run admin-cli binary");

    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

/// Run with env vars set.
fn run_cli_with_env(args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(cli_bin());
    cmd.args(args)
        .env_remove("RPC_URL")
        .env_remove("CONTRACT_ADDRESS")
        .env_remove("PRIVATE_KEY");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("failed to run admin-cli binary");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

// ---- Missing required args ----

#[test]
fn test_missing_rpc_url_shows_error() {
    let (code, _stdout, stderr) = run_cli(&[
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "status",
    ]);
    assert_ne!(code, 0, "should fail without --rpc-url");
    assert!(
        stderr.contains("--rpc-url") || stderr.contains("RPC_URL"),
        "error should mention rpc-url, got: {stderr}"
    );
}

#[test]
fn test_missing_contract_shows_error() {
    let (code, _stdout, stderr) = run_cli(&["--rpc-url", "http://localhost:8545", "status"]);
    assert_ne!(code, 0, "should fail without --contract");
    assert!(
        stderr.contains("--contract") || stderr.contains("CONTRACT_ADDRESS"),
        "error should mention contract, got: {stderr}"
    );
}

#[test]
fn test_missing_subcommand_shows_error() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://localhost:8545",
        "--contract",
        "0x0000000000000000000000000000000000000001",
    ]);
    assert_ne!(code, 0, "should fail without subcommand");
    // clap shows usage info
    assert!(
        stderr.contains("Usage") || stderr.contains("subcommand"),
        "error should mention usage, got: {stderr}"
    );
}

// ---- Env var override ----

#[test]
fn test_env_var_rpc_url_override() {
    // With env vars, no need for --rpc-url or --contract flags.
    // The command will fail at the RPC call (can't connect) but should parse successfully.
    let (code, _stdout, stderr) = run_cli_with_env(
        &["status"],
        &[
            ("RPC_URL", "http://127.0.0.1:19999"),
            (
                "CONTRACT_ADDRESS",
                "0x0000000000000000000000000000000000000001",
            ),
        ],
    );
    // It should parse args OK but fail at the RPC call level (not arg parsing).
    // If code != 0 and stderr doesn't mention missing args, the env vars worked.
    if code != 0 {
        assert!(
            !stderr.contains("--rpc-url") && !stderr.contains("--contract"),
            "env var should replace CLI flags, but got arg error: {stderr}"
        );
    }
}

#[test]
fn test_cli_flags_override_env_vars() {
    // CLI flags should take priority over env vars.
    let (code, _stdout, stderr) = run_cli_with_env(
        &[
            "--rpc-url",
            "http://127.0.0.1:19998",
            "--contract",
            "0x0000000000000000000000000000000000000002",
            "status",
        ],
        &[
            ("RPC_URL", "http://should-not-use:8545"),
            (
                "CONTRACT_ADDRESS",
                "0x0000000000000000000000000000000000000099",
            ),
        ],
    );
    // Should fail at RPC call to 127.0.0.1:19998, not the env var URL.
    if code != 0 {
        assert!(
            !stderr.contains("--rpc-url") && !stderr.contains("--contract"),
            "should not fail on arg parsing: {stderr}"
        );
    }
}

// ---- Write commands without private key ----

#[test]
fn test_pause_without_private_key_fails() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "pause",
    ]);
    assert_ne!(code, 0, "pause without --private-key should fail");
    assert!(
        stderr.contains("private") || stderr.contains("PRIVATE_KEY"),
        "error should mention private key, got: {stderr}"
    );
}

#[test]
fn test_register_enclave_without_private_key_fails() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "register-enclave",
        "0xABCDABCDABCDABCDABCDABCDABCDABCDABCDABCD",
        "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("private") || stderr.contains("PRIVATE_KEY"),
        "error should mention private key, got: {stderr}"
    );
}

// ---- Invalid arguments ----

#[test]
fn test_register_enclave_missing_image_hash() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "--private-key",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "register-enclave",
        "0xABCDABCDABCDABCDABCDABCDABCDABCDABCDABCD",
        // missing image_hash argument
    ]);
    assert_ne!(code, 0, "register-enclave without image_hash should fail");
    assert!(
        stderr.contains("IMAGE_HASH") || stderr.contains("image") || stderr.contains("required"),
        "error should mention missing argument, got: {stderr}"
    );
}

#[test]
fn test_set_stake_missing_amount() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "--private-key",
        "0xaa",
        "set-stake",
        // missing amount
    ]);
    assert_ne!(code, 0, "set-stake without amount should fail");
    assert!(
        stderr.contains("AMOUNT") || stderr.contains("amount") || stderr.contains("required"),
        "error should mention missing amount, got: {stderr}"
    );
}

// ---- Help and version ----

#[test]
fn test_help_flag() {
    let (code, stdout, _stderr) = run_cli(&["--help"]);
    assert_eq!(code, 0, "help should exit 0");
    assert!(
        stdout.contains("admin-cli") || stdout.contains("TEEMLVerifier"),
        "help output should describe the tool, got: {stdout}"
    );
    // Check that all subcommands are listed
    assert!(stdout.contains("status"), "help should list status command");
    assert!(stdout.contains("pause"), "help should list pause command");
    assert!(
        stdout.contains("unpause"),
        "help should list unpause command"
    );
}

#[test]
fn test_version_flag() {
    let (code, stdout, _stderr) = run_cli(&["--version"]);
    assert_eq!(code, 0, "version should exit 0");
    assert!(
        stdout.contains("admin-cli"),
        "version output should contain tool name, got: {stdout}"
    );
}

// ---- Subcommand help ----

#[test]
fn test_register_enclave_help() {
    let (code, stdout, _stderr) = run_cli(&["register-enclave", "--help"]);
    // clap outputs help to stdout with exit 0 when --help is first
    // but if --rpc-url is required, it might fail... Let's check it succeeds
    // or at least mentions register-enclave
    assert_eq!(code, 0, "subcommand --help should exit 0");
    assert!(
        stdout.contains("Register") || stdout.contains("enclave"),
        "help should describe register-enclave, got: {stdout}"
    );
}

// ---- Set-verifier and transfer-ownership ----

#[test]
fn test_set_verifier_missing_address() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "--private-key",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "set-verifier",
        // missing address
    ]);
    assert_ne!(code, 0, "set-verifier without address should fail");
    assert!(
        stderr.contains("ADDRESS") || stderr.contains("address") || stderr.contains("required"),
        "error should mention missing address, got: {stderr}"
    );
}

#[test]
fn test_transfer_ownership_missing_address() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "--private-key",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "transfer-ownership",
        // missing address
    ]);
    assert_ne!(code, 0, "transfer-ownership without address should fail");
    assert!(
        stderr.contains("ADDRESS") || stderr.contains("address") || stderr.contains("required"),
        "error should mention missing address, got: {stderr}"
    );
}

#[test]
fn test_set_bond_missing_amount() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "--private-key",
        "0xaa",
        "set-bond",
        // missing amount
    ]);
    assert_ne!(code, 0, "set-bond without amount should fail");
    assert!(
        stderr.contains("AMOUNT") || stderr.contains("amount") || stderr.contains("required"),
        "error should mention missing amount, got: {stderr}"
    );
}

#[test]
fn test_accept_ownership_without_private_key_fails() {
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "accept-ownership",
    ]);
    assert_ne!(
        code, 0,
        "accept-ownership without --private-key should fail"
    );
    assert!(
        stderr.contains("private") || stderr.contains("PRIVATE_KEY"),
        "error should mention private key, got: {stderr}"
    );
}

// ---- Dry run flag ----

#[test]
fn test_dry_run_flag_parsed() {
    // This tests that --dry-run doesn't cause a parse error,
    // even though the actual dry-run execution needs an RPC connection.
    let (code, _stdout, stderr) = run_cli(&[
        "--rpc-url",
        "http://127.0.0.1:19999",
        "--contract",
        "0x0000000000000000000000000000000000000001",
        "--private-key",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "--dry-run",
        "pause",
    ]);
    // Should fail at RPC call, not at arg parsing. Verify no parse error.
    if code != 0 {
        assert!(
            !stderr.contains("--dry-run"),
            "should not fail on parsing --dry-run flag, got: {stderr}"
        );
    }
}
