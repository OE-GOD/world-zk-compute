# zkml-verifier

Chain-agnostic GKR+Hyrax zero-knowledge proof verifier for ML inference.

This crate implements a complete GKR (Goldwasser-Kalai-Rothblum) interactive
proof verifier over BN254 field arithmetic, paired with a Hyrax polynomial
commitment scheme. It can verify proofs produced by the
[Remainder](https://github.com/OE-GOD/world-zk-compute) prover for
machine-learning inference circuits (e.g., XGBoost tree ensembles) without
any blockchain dependencies.

## Quick start

```rust
use zkml_verifier::{verify, ProofBundle};

let bundle = ProofBundle::from_file("proof_bundle.json").unwrap();
let result = verify(&bundle).unwrap();
assert!(result.verified);
println!("circuit hash: 0x{}", hex::encode(result.circuit_hash));
```

### Low-level API

If you already have raw byte slices for the proof, generators, and circuit
description, you can skip the JSON bundle layer:

```rust
use zkml_verifier::verify_raw;

let result = verify_raw(&proof_bytes, &gens_bytes, &circuit_desc_bytes).unwrap();
assert!(result.verified);
```

## Feature flags

| Feature | Description |
|---------|-------------|
| `wasm`  | Enables `wasm-bindgen` exports for browser/Node.js usage via `wasm-pack` |
| `ffi`   | Enables C-ABI FFI functions (`zkml_verify_json`, `zkml_verify_raw`, etc.) |

### WASM

```sh
wasm-pack build --target web crates/zkml-verifier -- --features wasm
```

```js
import init, { verify_proof_json } from './zkml_verifier.js';
await init();
const result = verify_proof_json(bundleJsonString);
console.log(result.verified, result.circuit_hash);
```

### FFI (C/Python/Node.js)

Build the shared library:

```sh
cargo build --release --features ffi -p zkml-verifier
```

The resulting `libzkml_verifier.so` / `libzkml_verifier.dylib` exposes:

- `zkml_verify_json(json_ptr, error_out) -> i32`
- `zkml_verify_file(path_ptr, error_out) -> i32`
- `zkml_verify_raw(proof_ptr, proof_len, ..., error_out) -> i32`
- `zkml_free_string(ptr)`
- `zkml_version() -> *const c_char`

## CLI

The crate also ships a small binary for command-line verification:

```sh
cargo install zkml-verifier
zkml-verifier verify proof_bundle.json
zkml-verifier verify proof_bundle.json --hybrid --json
```

## License

Apache-2.0. See [LICENSE](../../LICENSE) in the repository root.

## Links

- [Repository](https://github.com/OE-GOD/world-zk-compute)
- [Documentation](https://docs.rs/zkml-verifier)
