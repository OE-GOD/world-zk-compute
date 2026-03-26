//! Fuzz target for ProofBundle JSON deserialization — feeds arbitrary bytes
//! as potential JSON to serde_json::from_str::<ProofBundle>(). Must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zkml_verifier::ProofBundle;

fuzz_target!(|data: &[u8]| {
    // Try to interpret the bytes as UTF-8 and parse as JSON
    if let Ok(s) = std::str::from_utf8(data) {
        // Deserialization must not panic
        let result = serde_json::from_str::<ProofBundle>(s);

        // If deserialization succeeds, also exercise the accessor methods
        if let Ok(bundle) = result {
            let _ = bundle.proof_data();
            let _ = bundle.public_inputs_data();
            let _ = bundle.gens_data();
            let _ = bundle.metadata();
        }
    }

    // Also try ProofBundle::from_json directly (it returns Result)
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = ProofBundle::from_json(s);
    }
});
