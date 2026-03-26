# ZKML Verifier Web Demo

A single-page web application for verifying GKR+Hyrax zero-knowledge proofs in the browser using the WASM verifier.

## Quick Start

### 1. Build the WASM module

From the project root:

```bash
wasm-pack build --target web crates/zkml-verifier -- --features wasm
cp -r crates/zkml-verifier/pkg web/pkg
```

### 2. Serve locally

```bash
cd web
python3 -m http.server 8000
```

Then open [http://localhost:8000](http://localhost:8000).

### How to use

1. Click **Load Sample Proof** to load the pre-generated XGBoost proof bundle.
2. Review the proof metadata (circuit hash, layer counts, sizes).
3. Click **Verify Proof** to run full GKR+Hyrax verification in your browser.
4. Expand **Raw Proof Data** to inspect the hex-encoded proof and circuit description.

You can also upload your own proof bundle JSON file.

## Proof Bundle Format

The verifier expects a JSON file with these fields:

```json
{
  "proof_hex": "0x52454d31...",
  "gens_hex": "0x...",
  "dag_circuit_description": { ... },
  "public_inputs_hex": "0x...",
  "circuit_hash": "0x...",
  "model_hash": "0x...",
  "timestamp": 1711929600,
  "prover_version": "0.1.0"
}
```

Required fields: `proof_hex`, `gens_hex`, `dag_circuit_description`.

## Deploy to GitHub Pages

1. Build the WASM module and copy `pkg/` into `web/`:

   ```bash
   wasm-pack build --target web crates/zkml-verifier -- --features wasm
   cp -r crates/zkml-verifier/pkg web/pkg
   ```

2. Push the `web/` directory. In your repository settings, set GitHub Pages source to the `web/` folder on your branch.

   Or use a custom workflow (`.github/workflows/pages.yml`):

   ```yaml
   name: Deploy Web Demo
   on:
     push:
       branches: [main]
       paths: [web/**, crates/zkml-verifier/**]
   jobs:
     deploy:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v4
         - uses: dtolnay/rust-toolchain@stable
           with:
             targets: wasm32-unknown-unknown
         - run: cargo install wasm-pack
         - run: wasm-pack build --target web crates/zkml-verifier -- --features wasm
         - run: cp -r crates/zkml-verifier/pkg web/pkg
         - uses: peaceiris/actions-gh-pages@v3
           with:
             github_token: ${{ secrets.GITHUB_TOKEN }}
             publish_dir: ./web
   ```

## Architecture

```
web/
  index.html         Single-page app (no framework)
  style.css          Styling with dark/light theme
  app.js             Application logic (ES module)
  sample_proof.json  Pre-generated XGBoost proof bundle
  pkg/               WASM module (built by wasm-pack, not committed)
  README.md          This file
```

The WASM module (`pkg/`) is built from `crates/zkml-verifier` with the `wasm` feature flag. It exposes:

- `verify_proof_json(json: string) -> { verified, circuit_hash, error }` - Full verification
- `version() -> string` - Library version

All verification runs client-side. No data leaves the browser.
