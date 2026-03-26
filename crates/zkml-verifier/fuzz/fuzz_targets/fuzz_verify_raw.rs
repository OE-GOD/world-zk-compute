//! Fuzz target for verify_raw() — feeds random bytes as proof, generators,
//! and circuit description. The verifier must never panic; it should always
//! return Ok or Err.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zkml_verifier::verify_raw;

fuzz_target!(|data: &[u8]| {
    // Need at least 3 bytes so we can split into 3 non-empty slices
    if data.len() < 3 {
        return;
    }

    // Use first two bytes as split points (mod length) to divide data
    // into proof, generators, and circuit_desc portions
    let split1 = (data[0] as usize % (data.len() - 1)) + 1;
    let remaining = data.len() - split1;
    let split2 = if remaining > 1 {
        split1 + (data[1] as usize % (remaining - 1)) + 1
    } else {
        split1 + 1.min(remaining)
    };

    let proof = &data[..split1.min(data.len())];
    let gens = &data[split1.min(data.len())..split2.min(data.len())];
    let circuit_desc = &data[split2.min(data.len())..];

    // verify_raw must never panic — it wraps inner panics with catch_unwind
    let _ = verify_raw(proof, gens, circuit_desc);
});
