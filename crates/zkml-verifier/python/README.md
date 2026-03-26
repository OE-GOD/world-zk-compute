# zkml-verifier

Python wrapper for the ZKML proof verifier. Verifies GKR+Hyrax zero-knowledge proofs for ML inference on BN254.

## Installation

```bash
pip install zkml-verifier
```

**Prerequisites**: Rust toolchain (install via [rustup.rs](https://rustup.rs/)) is required for building from source.

## Quick Start

```python
from zkml_verifier import verify_bundle, verify_file, verify_json

# Verify from a file
result = verify_file("proof_bundle.json")
print(f"Verified: {result['verified']}")

# Verify from a Python dict
bundle = {
    "proof_hex": "0x...",
    "gens_hex": "0x...",
    "dag_circuit_description": { ... }
}
result = verify_bundle(bundle)

# Verify from a raw JSON string
result = verify_json('{"proof_hex": "0x...", ...}')
```

## API Reference

### `verify_file(path: str) -> dict`

Verify a proof bundle from a JSON file path.

### `verify_bundle(bundle: dict) -> dict`

Verify a proof bundle from a Python dictionary.

### `verify_json(json_str: str) -> dict`

Verify a proof bundle from a raw JSON string.

### `version() -> str`

Return the native library version string.

### Return Value

All verify functions return a dict:
```python
{"verified": True, "error": None}   # success
{"verified": False, "error": None}  # verification failed (proof invalid)
```

On fatal errors (malformed input, missing fields), a `VerifyError` exception is raised.

## Development

```bash
# Build the Rust library
cargo build --release -p zkml-verifier

# Install in development mode
cd crates/zkml-verifier/python
pip install -e .

# Run tests
pytest tests/
```

## Environment Variables

- `ZKML_LIB_DIR` -- Directory containing the compiled shared library
- `ZKML_SKIP_BUILD` -- Skip Rust compilation during pip install (use pre-built library)
- `CARGO_TARGET_DIR` -- Override Cargo target directory

## License

Apache-2.0
