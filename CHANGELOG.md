# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Production hardening phase 2: fuzz tests, invariant tests, improved error handling
- Monitoring stack: Prometheus scrape configs, Grafana dashboards (operator, enclave, indexer, Sepolia)
- Comprehensive documentation: architecture, threat model, runbook, troubleshooting, upgrade guide

## [2026-03-13] - Production Hardening and Documentation

### Added
- CI workflows for benchmarks, security audit, SDK publishing, and E2E smoke tests
- LightGBM precompute support in Python SDK
- Gas profiling tests and contract size checks

### Changed
- Improved error handling across operator, enclave, and SDK layers

## [2026-03-10] - DAG Batch Verifier, Groth16 Hybrid, and Stylus Port

### Added
- DAG batch verifier (DAGBatchVerifier.sol) for multi-tx GKR verification under 30M gas per tx
- DAG hybrid Groth16 verification with on-chain Poseidon transcript replay
- Stylus WASM port of GKR DAG verifier (52KB raw, 24.5KB Brotli compressed)

### Changed
- 88-layer XGBoost verification split into 15 transactions (1 setup + 11 continue + 3 finalize)
- 166 Solidity tests, 14 Go tests, 34 Rust tests passing

## [2026-03-06] - Operator Service and TEE Production Path

### Added
- Operator service with auto-dispute detection, circuit breaker, and webhook notifications
- AWS Nitro enclave attestation with P-384 certificate chain verification
- Admin CLI for contract management and deployment operations
- Python and TypeScript SDK feature parity (async client, batch verifier, event watcher)

### Changed
- TEE architecture redesigned: happy path at ~$0.0001/inference with ZK dispute fallback

## [2026-03-01] - XGBoost Circuit and GKR On-Chain Verifier

### Added
- XGBoost tree inference circuit with leaf selection and comparison verification (Phase 1a+1b)
- GKR + Hyrax on-chain verifier (PoseidonSponge, SumcheckVerifier, HyraxVerifier, GKRVerifier)
- DAG verifier for arbitrary circuit topologies (GKRDAGVerifier.sol)
- XGBoost JSON model import with automatic circuit generation

### Changed
- GKR direct verification benchmarked at ~7.6M gas (simple) and ~252M gas (88-layer XGBoost DAG)

## [2026-02-20] - Core Infrastructure

### Added
- Core contracts: TEEMLVerifier, ExecutionEngine, ProgramRegistry with UUPS upgradeability
- risc0-zkvm v3.0 prover with pre-compiled guest program binaries
- Foundry test suite for on-chain verification
- Python, TypeScript, and Rust SDKs
- Docker Compose for local development and GPU-accelerated proving
- Sepolia testnet deployment scripts with auto-funding
- E2E test script supporting local and Sepolia networks
