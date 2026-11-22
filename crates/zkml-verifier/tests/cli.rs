//! Tests for the `zkml-verifier` binary: argument handling, exit codes and
//! `--json` output on success and failure.
//!
//! Cases that run a full verification are `#[ignore]`d (about 2 s in release,
//! about a minute in debug). Run them with:
//!   cargo test -p zkml-verifier --release --test cli -- --ignored

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zkml-verifier"))
}

fn fixture_path() -> String {
    format!(
        "{}/../../contracts/test/fixtures/phase1a_dag_fixture.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture() -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_path()).unwrap()).unwrap()
}

/// A fresh scratch directory under the system temp dir.
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("zkml-verifier-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

/// Parse stdout as one JSON value (fails the test if it is not valid JSON).
fn stdout_json(out: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(out))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {:?}", stdout(out)))
}

/// Write the fixture as a bundle with the proof cut short, which the decoder
/// rejects immediately.
fn write_truncated_bundle(dir: &std::path::Path) -> String {
    let f = fixture();
    let proof_hex = f["proof_hex"].as_str().unwrap();
    let bundle = serde_json::json!({
        "proof_hex": &proof_hex[..2002],
        "gens_hex": f["gens_hex"],
        "dag_circuit_description": f["dag_circuit_description"],
    });
    let path = dir.join("truncated.json");
    std::fs::write(&path, serde_json::to_vec(&bundle).unwrap()).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn help_and_version_exit_zero() {
    for args in [
        vec!["--help"],
        vec!["-h"],
        vec!["help"],
        vec!["verify", "--help"],
        vec!["bundle", "--help"],
    ] {
        let out = bin().args(&args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(text.contains("Usage:"), "{args:?}: {text}");
        // Every flag the parser accepts is documented.
        for flag in [
            "--hybrid",
            "--json",
            "--proof",
            "--gens",
            "--desc",
            "--pub-inputs",
            "--output",
            "--compress",
        ] {
            assert!(
                text.contains(flag),
                "{args:?}: usage does not mention {flag}"
            );
        }
    }

    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        format!("zkml-verifier {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn bad_invocations_exit_nonzero() {
    for args in [
        vec![],
        vec!["frobnicate"],
        vec!["verify"],
        vec!["verify", "--json"],
        vec!["verify", "a.json", "--bogus"],
        vec!["verify", "a.json", "b.json"],
        vec!["verify", "/nonexistent/bundle.json"],
        vec!["bundle"],
        vec!["bundle", "--bogus"],
        vec!["bundle", "--proof"],
        vec![
            "bundle",
            "--proof",
            "/nonexistent/p",
            "--gens",
            "/nonexistent/g",
        ],
    ] {
        let out = bin().args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(stdout(&out).is_empty(), "{args:?}: {}", stdout(&out));
        assert!(!stderr(&out).is_empty(), "{args:?}");
    }
}

#[test]
fn json_failure_output_is_valid_json() {
    let dir = scratch("json-failure");
    let truncated = write_truncated_bundle(&dir);
    let not_json = dir.join("not_json.json");
    std::fs::write(&not_json, b"this is not json").unwrap();
    let empty_desc = dir.join("empty_desc.json");
    let f = fixture();
    std::fs::write(
        &empty_desc,
        serde_json::to_vec(&serde_json::json!({
            "proof_hex": f["proof_hex"],
            "gens_hex": f["gens_hex"],
            "dag_circuit_description": {},
        }))
        .unwrap(),
    )
    .unwrap();

    for path in [
        truncated.as_str(),
        not_json.to_str().unwrap(),
        empty_desc.to_str().unwrap(),
        "/nonexistent/bundle.json",
    ] {
        // The flag may come before or after the path.
        for args in [
            vec!["verify", path, "--json"],
            vec!["verify", "--json", path],
        ] {
            let out = bin().args(&args).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{args:?}");
            let v = stdout_json(&out);
            assert_eq!(v["verified"], false, "{args:?}");
            assert!(!v["error"].as_str().unwrap().is_empty(), "{args:?}");
            // One line on stderr: no panic message, no backtrace hint.
            assert_eq!(stderr(&out).trim().lines().count(), 1, "{}", stderr(&out));
        }

        let out = bin()
            .args(["verify", path, "--hybrid", "--json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "hybrid {path}");
        assert!(!stdout_json(&out)["error"].as_str().unwrap().is_empty());
    }

    // Without --json nothing is written to stdout on failure.
    let out = bin().args(["verify", &truncated]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bundle_assembles_a_loadable_bundle() {
    let dir = scratch("bundle");
    let f = fixture();
    let proof_hex = f["proof_hex"].as_str().unwrap();
    let gens_hex = f["gens_hex"].as_str().unwrap();

    // Proof as hex text, generators as raw binary, public inputs as hex text.
    let proof = dir.join("proof.hex");
    let gens = dir.join("gens.bin");
    let desc = dir.join("desc.json");
    let pub_inputs = dir.join("pub.hex");
    std::fs::write(&proof, format!("{proof_hex}\n")).unwrap();
    std::fs::write(&gens, hex::decode(&gens_hex[2..]).unwrap()).unwrap();
    std::fs::write(
        &desc,
        serde_json::to_vec(&f["dag_circuit_description"]).unwrap(),
    )
    .unwrap();
    std::fs::write(&pub_inputs, f["public_inputs_hex"].as_str().unwrap()).unwrap();

    let common = [
        "bundle",
        "-p",
        proof.to_str().unwrap(),
        "--gens",
        gens.to_str().unwrap(),
        "-d",
        desc.to_str().unwrap(),
        "--pub-inputs",
        pub_inputs.to_str().unwrap(),
    ];

    // Plain JSON to a file.
    let plain = dir.join("bundle.json");
    let out = bin()
        .args(common)
        .args(["-o", plain.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let loaded = zkml_verifier::ProofBundle::from_file(plain.to_str().unwrap()).unwrap();
    assert_eq!(loaded.proof_hex, proof_hex);
    assert_eq!(loaded.gens_hex, gens_hex);
    assert_eq!(
        loaded.public_inputs_hex,
        f["public_inputs_hex"].as_str().unwrap()
    );
    assert_eq!(loaded.dag_circuit_description, f["dag_circuit_description"]);

    // --compress adds the .gz suffix and the result loads transparently.
    let out = bin()
        .args(common)
        .args(["--output", plain.to_str().unwrap(), "--compress"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let gz = dir.join("bundle.json.gz");
    let raw = std::fs::read(&gz).unwrap();
    assert!(zkml_verifier::ProofBundle::is_compressed(&raw));
    let loaded = zkml_verifier::ProofBundle::from_file(gz.to_str().unwrap()).unwrap();
    assert_eq!(loaded.proof_hex, proof_hex);

    // No -o: JSON on stdout.
    let out = bin().args(common).output().unwrap();
    assert!(out.status.success());
    assert_eq!(stdout_json(&out)["proof_hex"], proof_hex);

    // No -o with --compress: gzip on stdout.
    let out = bin().args(common).arg("--compress").output().unwrap();
    assert!(out.status.success());
    let loaded = zkml_verifier::ProofBundle::from_bytes(&out.stdout).unwrap();
    assert_eq!(loaded.gens_hex, gens_hex);

    // Unwritable output path.
    let out = bin()
        .args(common)
        .args(["-o", "/nonexistent-dir/bundle.json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Full verification of the fixture through the CLI, in every output mode.
#[test]
#[ignore] // ~2s per verification in release, ~1 min in debug
fn verify_fixture_all_modes() {
    let path = fixture_path();
    let hash = fixture()["circuit_hash_raw"].as_str().unwrap().to_string();

    let out = bin().args(["verify", &path, "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let v = stdout_json(&out);
    assert_eq!(v["verified"], true);
    assert_eq!(v["circuit_hash"], hash.as_str());

    // Legacy form without the subcommand.
    let out = bin().args([&path, "--json"]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(stdout_json(&out)["verified"], true);

    let out = bin().args(["verify", &path]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        format!("verified: true, circuit_hash: {hash}")
    );

    let out = bin()
        .args(["verify", &path, "--hybrid", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = stdout_json(&out);
    assert_eq!(v["circuit_hash"], hash.as_str());
    assert_eq!(v["num_compute_layers"], 88);
    assert!(v["transcript_digest"].as_str().unwrap().starts_with("0x"));
}

/// A proof with one flipped byte is rejected with exit status 1 and a JSON
/// error, and prints no panic message.
#[test]
#[ignore] // verifies about half of the layers before the flipped byte is reached
fn verify_tampered_proof_is_rejected() {
    let dir = scratch("tampered");
    let f = fixture();
    let mut proof = hex::decode(&f["proof_hex"].as_str().unwrap()[2..]).unwrap();
    let mid = proof.len() / 2;
    proof[mid] ^= 0xff;
    let path = dir.join("tampered.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "proof_hex": format!("0x{}", hex::encode(&proof)),
            "gens_hex": f["gens_hex"],
            "dag_circuit_description": f["dag_circuit_description"],
        }))
        .unwrap(),
    )
    .unwrap();

    let out = bin()
        .args(["verify", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = stdout_json(&out);
    assert_eq!(v["verified"], false);
    assert!(v["error"].as_str().unwrap().contains("verification failed"));
    assert!(!stderr(&out).contains("panicked"), "{}", stderr(&out));

    let _ = std::fs::remove_dir_all(&dir);
}
