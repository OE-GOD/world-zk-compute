# zkml-verifier

Python wrapper for the ZKML proof verifier. Verifies GKR+Hyrax zero-knowledge proofs for ML inference on BN254.

## Installation

The package is not published on PyPI. Install it from a checkout of the repository:

```bash
pip install ./crates/zkml-verifier/python
```

This runs `cargo build --release` for the Rust library and bundles the shared library into the package.

**Prerequisites**: Rust toolchain (install via [rustup.rs](https://rustup.rs/)).

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

All verify functions return a dict when the proof verifies:
```python
{"verified": True, "error": None}
```

A rejected proof raises `VerifyError`, as does malformed input (missing fields, bad hex, a truncated proof):
```python
from zkml_verifier import VerifyError, verify_file

try:
    verify_file("proof_bundle.json")
except VerifyError as e:
    print("rejected:", e)   # e.g. "verify error: verification failed: sumcheck failed"
```

`verify_file` raises `FileNotFoundError` if the path does not exist.

## Development

```bash
# Build the Rust library
cargo build --release -p zkml-verifier

# Install in development mode
cd crates/zkml-verifier/python
pip install -e .

# Run tests (the fixture tests need the release build: about 2 s each, minutes with a debug build)
pip install pytest
pytest tests/
```

## Environment Variables

- `ZKML_LIB_DIR` -- Directory containing the compiled shared library
- `ZKML_SKIP_BUILD` -- Skip Rust compilation during pip install (use pre-built library)
- `CARGO_TARGET_DIR` -- Cargo target directory to build into (pip install) and to search for the library (at import, before the bundled copy)

## License

Apache-2.0
