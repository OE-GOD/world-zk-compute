//! WebAssembly bindings for zkml-verifier via wasm-bindgen.
//!
//! Compile with:
//! ```sh
//! wasm-pack build --target web crates/zkml-verifier
//! ```
//!
//! Usage from JavaScript:
//! ```js
//! import init, { verify_proof_json } from './zkml_verifier.js';
//! await init();
//! const result = verify_proof_json(bundleJsonString);
//! console.log(result.verified, result.circuit_hash);
//! ```

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// WASM-exported verification result.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub struct WasmVerifyResult {
    verified: bool,
    circuit_hash: String,
    error: Option<String>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl WasmVerifyResult {
    #[wasm_bindgen(getter)]
    pub fn verified(&self) -> bool {
        self.verified
    }

    #[wasm_bindgen(getter)]
    pub fn circuit_hash(&self) -> String {
        self.circuit_hash.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }
}

/// Verify a proof bundle from a JSON string.
///
/// Returns a `WasmVerifyResult` with `verified`, `circuit_hash`, and optional `error`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn verify_proof_json(json: &str) -> WasmVerifyResult {
    let bundle = match crate::ProofBundle::from_json(json) {
        Ok(b) => b,
        Err(e) => {
            return WasmVerifyResult {
                verified: false,
                circuit_hash: String::new(),
                error: Some(format!("parse error: {}", e)),
            };
        }
    };

    match crate::verify(&bundle) {
        Ok(result) => WasmVerifyResult {
            verified: result.verified,
            circuit_hash: hex::encode(result.circuit_hash),
            error: None,
        },
        Err(e) => WasmVerifyResult {
            verified: false,
            circuit_hash: String::new(),
            error: Some(format!("verify error: {}", e)),
        },
    }
}

/// Return the library version.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
