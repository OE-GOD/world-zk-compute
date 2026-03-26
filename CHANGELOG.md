# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-03-26

### Added

#### Core Verifier
- `zkml-verifier` crate: standalone GKR+Hyrax DAG proof verification (BN254)
- `verify()` and `verify_hybrid()` API for full and transcript-only verification
- `verify_raw()` low-level API for direct byte-slice verification
- `ProofBundle` JSON format with metadata (model_hash, timestamp, prover_version)
- C-FFI bindings (`zkml_verify_json`, `zkml_verify_raw`, `zkml_free_string`)
- WASM bindings via `wasm-bindgen` (browser verification)
- Python wrapper (`pip install zkml-verifier`) with ctypes FFI
- npm package (`@worldzk/verifier`) for JavaScript/TypeScript
- CLI tool: `zkml-verifier verify <bundle.json> [--hybrid] [--json]`
- CLI tool: `zkml-verifier bundle --proof <hex> --gens <hex> --desc <json>`
- Criterion benchmarks for verification performance

#### Verifier REST API
- `POST /verify` — full GKR verification from ProofBundle JSON
- `POST /verify/batch` — batch verification (up to 100 bundles)
- `POST /verify/hybrid` — transcript-only verification
- Circuit registration and warm verification (`/circuits`)
- Rate limiting, API key auth, TLS/mTLS support
- Multi-tenant API key management
- OpenAPI 3.1 specification

#### TEE Enclave
- AWS Nitro NSM real attestation with graceful fallback to mock
- Multi-model support (load/unload models at runtime)
- EIP-712 ECDSA attestation with replay protection (chain_id, nonce, timestamp)
- Rate limiting, CORS, request timeout configuration

#### Operator Service
- Submit-watch-dispute lifecycle automation
- Proof pre-verification using zkml-verifier before on-chain submission
- Proof archive storage (append-only, date-partitioned)
- Proof expiry and rotation policies
- Audit log export (CSV, JSON, JSONL/NDJSON)
- Prometheus metrics and webhook notifications

#### ML Model Support
- XGBoost decision tree ensemble inference + GKR circuit
- LightGBM model parser and inference
- Random forest circuit compiler (scikit-learn JSON format)
- Logistic regression circuit (dot product + sign check, full prove-and-verify)
- Auto-detection of model format from JSON structure
- `--bundle` flag for self-contained ProofBundle output

#### On-Chain Contracts
- `TEEMLVerifier`: submit-challenge-dispute lifecycle with EIP-712 verification
- `RemainderVerifier`: GKR/Hyrax/Groth16 proof verification
- DAG batch verifier: multi-tx verification for L1 (15 txs for 88-layer XGBoost)
- Stylus WASM verifier: ~5-25M gas on Arbitrum
- Stylus + Groth16 hybrid: ~3-6M gas target
- Chunked EC Groth16: split large EC circuits across multiple proofs
- UUPS proxy + pausable + timelock admin

#### Infrastructure
- Docker Compose for bank demo (verifier API + optional Anvil)
- Fly.io deployment config + CI workflow
- GitHub Actions: Rust CI, Solidity CI, Stylus CI, verifier CI
- PyPI and npm publish workflows
- Bank demo CLI with credit scoring model

#### Documentation
- API Reference (contracts + services + CLI)
- Proof Format Reference (all wire formats)
- Integration Guide (patterns, chain-specific notes, monitoring)
- Security Model (defense-in-depth, threat model)
- Audit Trail (on-chain events, off-chain logs, SIEM)
- Quickstart tutorial (Python, JavaScript, Rust, REST API)
- Architecture overview (5 verification paths)
- Benchmarks (native verification timing + on-chain gas costs)

### Security
- Poseidon sponge for Fiat-Shamir transcript (BN254 PSE/Scroll constants)
- MiMC batch challenge computed in-circuit (prevents malicious prover)
- OpsDigest integrity check across chunked Groth16 proofs
- Replay protection: chain_id + nonce + timestamp in attestation
- Rate limiting on all POST endpoints
- SSRF protection in operator service
