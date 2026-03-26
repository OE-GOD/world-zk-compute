//! Fuzz target for ProofDecoder — feeds random bytes to the safe decoder
//! methods (try_decode_proof_for_dag, try_decode_pedersen_gens, try_read_u256).
//! These return Result instead of panicking, so they should handle all inputs
//! gracefully without crashing.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Test 1: try_decode_pedersen_gens (safe version)
    let _ = zkml_verifier::decode::ProofDecoder::try_decode_pedersen_gens(data);

    // Test 2: try_read_u256 on individual values
    {
        let mut dec = zkml_verifier::decode::ProofDecoder::new(data);
        // Read as many U256 values as possible
        loop {
            match dec.try_read_u256() {
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    }

    // Test 3: Full try_decode_proof_for_dag
    {
        let mut dec = zkml_verifier::decode::ProofDecoder::new(data);
        let _ = dec.try_decode_proof_for_dag();
    }

    // Test 4: decode_embedded_public_inputs via safe extraction
    // The safe path is used internally by try_decode_proof_for_dag,
    // but we also test direct construction with various offsets
    if data.len() >= 64 {
        let mut dec = zkml_verifier::decode::ProofDecoder::new(data);
        dec.set_offset(32);
        let _ = dec.try_read_u256();
    }
});
