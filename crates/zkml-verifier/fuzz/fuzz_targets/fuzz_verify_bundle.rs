//! Fuzz target for verify() on mutations of a valid proof.
//!
//! Each input flips bytes of the committed 88-layer fixture proof and verifies
//! the result under the fixture's own circuit description and generators. The
//! verifier must never panic, and a proof that differs from the fixture proof
//! in any byte must never come back as verified. A description of zero layers
//! would make every check vacuous, so the real description is used.
//!
//! A full verification takes about 2 s in release, so throughput is low;
//! inputs that fail early (in decoding or in the first layers) are fast.
#![no_main]
use std::sync::OnceLock;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use zkml_verifier::{verify, ProofBundle};

struct Fixture {
    proof: Vec<u8>,
    gens_hex: String,
    description: serde_json::Value,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let json: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../contracts/test/fixtures/phase1a_dag_fixture.json"
        ))
        .expect("fixture is valid JSON");
        let proof_hex = json["proof_hex"].as_str().expect("proof_hex");
        Fixture {
            proof: hex::decode(&proof_hex[2..]).expect("proof_hex is hex"),
            gens_hex: json["gens_hex"].as_str().expect("gens_hex").to_string(),
            description: json["dag_circuit_description"].clone(),
        }
    })
}

/// Byte flips applied to the fixture proof: each XORs `mask` into the byte
/// at `offset` (taken modulo the proof length).
#[derive(Debug, Arbitrary)]
struct FuzzInput {
    flips: Vec<(u32, u8)>,
}

fuzz_target!(
    // libfuzzer-sys installs a panic hook that aborts before unwinding, so a
    // check that fails by panicking inside verify()'s catch_unwind would be
    // reported as a crash. With the default hook back, those panics unwind
    // into verify() and become Err. A panic in this target's own code still
    // aborts, because it cannot unwind out of the extern "C" function that
    // fuzz_target! generates. A build with -Cpanic=abort would still report
    // every check that fails by panicking as a crash.
    init: drop(std::panic::take_hook()),
    |input: FuzzInput| {
        let f = fixture();
        let mut proof = f.proof.clone();
        for (offset, mask) in &input.flips {
            let i = *offset as usize % proof.len();
            proof[i] ^= mask;
        }
        // Flips that cancel out leave the valid proof, which may verify.
        if proof == f.proof {
            return;
        }

        let bundle = ProofBundle {
            proof_hex: format!("0x{}", hex::encode(&proof)),
            public_inputs_hex: String::new(),
            gens_hex: f.gens_hex.clone(),
            dag_circuit_description: f.description.clone(),
            model_hash: None,
            timestamp: None,
            prover_version: None,
            circuit_hash: None,
        };

        // verify() must return, never panic (it catches panics of the checks).
        if let Ok(result) = verify(&bundle) {
            assert!(
                !result.verified,
                "a proof that differs from the fixture proof was accepted"
            );
        }
    }
);
