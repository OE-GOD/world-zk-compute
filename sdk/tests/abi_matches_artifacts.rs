//! Every function, event and error the SDK declares in its `sol!` blocks must exist,
//! with the same signature, in the compiled contract (`contracts/out`).
//!
//! A declaration that drifts from the contract still compiles, but its selector or
//! topic no longer matches anything on-chain: calls revert and events never decode.
//!
//! What is compared is the name and the input types of every declared function,
//! event and error. State mutability, return types and the `indexed` flags of event
//! arguments are not compared.
//!
//! Prerequisite (same as `integration_tee.rs`):
//!   cd contracts && forge build --skip test --skip script

use std::collections::BTreeSet;
use std::path::PathBuf;

use world_zk_sdk::abi::{RemainderVerifier, TEEMLVerifier};
use world_zk_sdk::execution_engine::ExecutionEngine;
use world_zk_sdk::prover_registry::ProverRegistry;

/// Canonical ABI type of one artifact input (`tuple` expanded to its components).
fn canonical_type(param: &serde_json::Value) -> String {
    let ty = param["type"].as_str().expect("abi input without a type");
    match ty.strip_prefix("tuple") {
        Some(array_suffix) => {
            let components: Vec<String> = param["components"]
                .as_array()
                .expect("tuple without components")
                .iter()
                .map(canonical_type)
                .collect();
            format!("({}){array_suffix}", components.join(","))
        }
        None => ty.to_string(),
    }
}

/// Signatures of every entry of `kind` ("function", "event" or "error") in the artifact.
fn artifact_signatures(artifact: &str, kind: &str) -> BTreeSet<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/out")
        .join(artifact);
    assert!(
        path.exists(),
        "Artifact not found at {path:?}. \
         Run: cd contracts && forge build --skip test --skip script"
    );
    let json: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(&path).unwrap()).unwrap();
    json["abi"]
        .as_array()
        .expect("artifact without an abi array")
        .iter()
        .filter(|entry| entry["type"] == kind)
        .map(|entry| {
            let inputs: Vec<String> = entry["inputs"]
                .as_array()
                .expect("abi entry without inputs")
                .iter()
                .map(canonical_type)
                .collect();
            format!("{}({})", entry["name"].as_str().unwrap(), inputs.join(","))
        })
        .collect()
}

/// Assert that every SDK-declared signature of `kind` exists in the artifact.
fn assert_declared_in_artifact(artifact: &str, kind: &str, declared: &[&str]) {
    assert!(
        !declared.is_empty(),
        "the SDK declares no {kind}s for {artifact}"
    );
    let on_chain = artifact_signatures(artifact, kind);
    let missing: Vec<&str> = declared
        .iter()
        .copied()
        .filter(|sig| !on_chain.contains(*sig))
        .collect();
    assert!(
        missing.is_empty(),
        "{artifact}: the SDK declares {kind}s the contract does not have: {missing:?}"
    );
}

#[test]
fn remainder_verifier_bindings_match_artifact() {
    let artifact = "RemainderVerifier.sol/RemainderVerifier.json";
    assert_declared_in_artifact(
        artifact,
        "function",
        RemainderVerifier::RemainderVerifierCalls::SIGNATURES,
    );
    assert_declared_in_artifact(
        artifact,
        "event",
        RemainderVerifier::RemainderVerifierEvents::SIGNATURES,
    );
    assert_declared_in_artifact(
        artifact,
        "error",
        RemainderVerifier::RemainderVerifierErrors::SIGNATURES,
    );
}

#[test]
fn tee_ml_verifier_bindings_match_artifact() {
    let artifact = "TEEMLVerifier.sol/TEEMLVerifier.json";
    assert_declared_in_artifact(
        artifact,
        "function",
        TEEMLVerifier::TEEMLVerifierCalls::SIGNATURES,
    );
    assert_declared_in_artifact(
        artifact,
        "event",
        TEEMLVerifier::TEEMLVerifierEvents::SIGNATURES,
    );
}

#[test]
fn execution_engine_bindings_match_artifact() {
    let artifact = "ExecutionEngine.sol/ExecutionEngine.json";
    assert_declared_in_artifact(
        artifact,
        "function",
        ExecutionEngine::ExecutionEngineCalls::SIGNATURES,
    );
    assert_declared_in_artifact(
        artifact,
        "event",
        ExecutionEngine::ExecutionEngineEvents::SIGNATURES,
    );
    assert_declared_in_artifact(
        artifact,
        "error",
        ExecutionEngine::ExecutionEngineErrors::SIGNATURES,
    );
}

#[test]
fn prover_registry_bindings_match_artifact() {
    let artifact = "ProverRegistry.sol/ProverRegistry.json";
    assert_declared_in_artifact(
        artifact,
        "function",
        ProverRegistry::ProverRegistryCalls::SIGNATURES,
    );
    assert_declared_in_artifact(
        artifact,
        "event",
        ProverRegistry::ProverRegistryEvents::SIGNATURES,
    );
    assert_declared_in_artifact(
        artifact,
        "error",
        ProverRegistry::ProverRegistryErrors::SIGNATURES,
    );
}

/// The event watcher matches logs by hand-written topic strings; they must be
/// real `TEEMLVerifier` events too.
#[test]
fn event_watcher_topics_match_artifact() {
    use alloy::primitives::keccak256;
    use world_zk_sdk::event_watcher::{
        topic_result_challenged, topic_result_expired, topic_result_finalized,
        topic_result_submitted,
    };

    let on_chain: BTreeSet<_> =
        artifact_signatures("TEEMLVerifier.sol/TEEMLVerifier.json", "event")
            .iter()
            .map(|sig| keccak256(sig.as_bytes()))
            .collect();
    for (name, topic) in [
        ("ResultSubmitted", topic_result_submitted()),
        ("ResultChallenged", topic_result_challenged()),
        ("ResultFinalized", topic_result_finalized()),
        ("ResultExpired", topic_result_expired()),
    ] {
        assert!(
            on_chain.contains(&topic),
            "event watcher topic for {name} is not a TEEMLVerifier event"
        );
    }
}
