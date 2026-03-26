# Package Publishing Guide

This document explains how to build, validate, and publish the `zkml-verifier` packages to PyPI (Python) and npm (JavaScript/WASM).

## Overview

The `zkml-verifier` crate (`crates/zkml-verifier/`) produces three distributable artifacts:

| Target | Registry | Package Name | Install |
|--------|----------|-------------|---------|
| Python (ctypes FFI) | PyPI | `zkml-verifier` | `pip install zkml-verifier` |
| JavaScript (WASM) | npm | `zkml-verifier` | `npm install zkml-verifier` |
| Rust | crates.io | `zkml-verifier` | `cargo add zkml-verifier` |

## Prerequisites

- **Rust toolchain** (1.92.0+): `rustup update stable`
- **wasm-pack**: `cargo install wasm-pack` (for npm/WASM builds)
- **Python 3.8+** with `pip`: for PyPI wheel builds
- **Node.js 20+** with `npm`: for npm package validation

## PyPI Package (Python)

The Python package lives in `crates/zkml-verifier/python/`. It wraps the Rust `cdylib` via `ctypes` FFI, bundling the native shared library inside the wheel.

### Build Steps

```bash
# 1. Build the native shared library
cargo build --release --lib -p zkml-verifier

# 2. Build the Python wheel (skipping Rust build since we already built)
cd crates/zkml-verifier/python
pip install build wheel
ZKML_SKIP_BUILD=1 ZKML_LIB_DIR=../../target/release python -m build --wheel
```

The wheel is written to `crates/zkml-verifier/python/dist/`.

### Validate Locally

```bash
# Check wheel contents (should include __init__.py + native library)
unzip -l dist/*.whl

# Test in a clean virtualenv
python -m venv /tmp/test-zkml
source /tmp/test-zkml/bin/activate
pip install dist/*.whl
python -c "from zkml_verifier import version; print(version())"
python -c "from zkml_verifier import verify_json, VerifyError; print('OK')"
deactivate
rm -rf /tmp/test-zkml
```

### Publish to PyPI

```bash
pip install twine

# Check the package metadata
twine check dist/*

# Upload (requires PYPI_TOKEN)
twine upload dist/*
```

For CI, set the `PYPI_TOKEN` secret and use the workflow described below.

### Platform Wheels

The current wheel is tagged `py3-none-any` but contains a platform-specific native library. For production distribution across platforms, you should build separate wheels per platform (linux-x86_64, macos-arm64, macos-x86_64, windows-x86_64) using CI matrix builds or a tool like `cibuildwheel`.

### Environment Variables

| Variable | Purpose |
|----------|---------|
| `ZKML_SKIP_BUILD` | Skip `cargo build` during wheel build (use pre-built library) |
| `ZKML_LIB_DIR` | Directory containing the pre-built `libzkml_verifier.{so,dylib,dll}` |
| `CARGO_TARGET_DIR` | Custom Rust build output directory |

## npm Package (JavaScript/WASM)

The WASM package is built with `wasm-pack` from the same `crates/zkml-verifier/` crate, using the `wasm` feature flag.

### Build Steps

```bash
cd crates/zkml-verifier

# Build WASM package (output in pkg/)
wasm-pack build --target web --features wasm
```

This produces the `pkg/` directory containing:
- `zkml_verifier.js` -- JavaScript glue code
- `zkml_verifier_bg.wasm` -- compiled WebAssembly binary (~260KB)
- `zkml_verifier.d.ts` -- TypeScript type declarations
- `package.json` -- npm package manifest

### Validate Locally

```bash
# Check package.json
cat pkg/package.json

# Dry-run npm pack to see what would be published
cd pkg
npm pack --dry-run
```

### Publish to npm

```bash
cd crates/zkml-verifier/pkg

# Login (interactive, or use NPM_TOKEN in CI)
npm login

# Publish
npm publish --access public
```

### JavaScript Usage

```javascript
import init, { verify_proof_json, version } from 'zkml-verifier';

// Initialize WASM
await init();

// Check version
console.log('Version:', version());

// Verify a proof
const result = verify_proof_json(proofBundleJsonString);
console.log('Verified:', result.verified);
console.log('Circuit hash:', result.circuit_hash);
if (result.error) {
  console.error('Error:', result.error);
}
result.free(); // Free WASM memory
```

## Rust Crate (crates.io)

The Rust crate is published directly from `crates/zkml-verifier/`.

### Validate

```bash
cd crates/zkml-verifier
cargo package --list     # Show files that would be included
cargo publish --dry-run  # Validate without publishing
```

### Publish

```bash
# Requires CARGO_REGISTRY_TOKEN
cargo publish -p zkml-verifier
```

## CI Workflows

### Release Workflow (`.github/workflows/release.yml`)

Triggered by pushing a `v*` tag (e.g., `v0.1.0`). Runs tests, builds Docker images, creates a GitHub Release, and publishes:
- **npm**: TypeScript SDK to npm (requires `NPM_TOKEN` secret)
- **PyPI**: Python SDK to PyPI (requires `PYPI_TOKEN` secret)

### SDK Publish Workflow (`.github/workflows/sdk-publish.yml`)

Triggered by pushing an `sdk-v*` tag or manual dispatch. More comprehensive than the release workflow:

1. **Version consistency check** -- verifies TypeScript, Python, and Rust SDK versions match
2. **npm publish** -- builds, tests, and publishes TypeScript SDK
3. **PyPI publish** -- builds, tests, and publishes Python SDK
4. **crates.io publish** -- builds, tests, and publishes Rust SDK
5. **GitHub Release** -- creates a release with changelog

Supports dry-run mode via workflow dispatch (default: dry-run=true).

### Required Secrets

| Secret | Registry | How to obtain |
|--------|----------|--------------|
| `NPM_TOKEN` | npm | `npm token create` or npm website > Access Tokens |
| `PYPI_TOKEN` | PyPI | PyPI > Account settings > API tokens |
| `CARGO_REGISTRY_TOKEN` | crates.io | `cargo login` or crates.io > Account Settings > API Tokens |

## Version Bumping

All three packages should have matching versions. Update these files:

1. `crates/zkml-verifier/Cargo.toml` -- `version = "X.Y.Z"`
2. `crates/zkml-verifier/python/pyproject.toml` -- `version = "X.Y.Z"`
3. `crates/zkml-verifier/src/ffi.rs` -- `VERSION` static string
4. `crates/zkml-verifier/python/zkml_verifier/__init__.py` -- `__version__`

Note: The WASM/npm package version is automatically derived from `Cargo.toml` by `wasm-pack`.

## Validation Checklist

Before publishing a new version:

- [ ] All three version strings match
- [ ] `cargo test -p zkml-verifier` passes
- [ ] Python wheel builds and installs in a clean venv
- [ ] `python -c "from zkml_verifier import version; print(version())"` prints correct version
- [ ] WASM builds with `wasm-pack build --target web --features wasm`
- [ ] `npm pack --dry-run` in `pkg/` shows correct files
- [ ] CI workflows pass on the branch
