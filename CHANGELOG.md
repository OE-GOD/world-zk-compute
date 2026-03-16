# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

_No unreleased changes._

## [2026-03-16] - Security Hardening, Custom Errors, and Quality Improvements (Phases 71-77)

### Security

- **Ownable2Step migration**: `ProverRegistry` and `ProverReputation` upgraded from `Ownable` to `Ownable2Step` for safer ownership transfers (Phase 77)
- **Reentrancy guards**: Added `nonReentrant` to `TEEMLVerifier.extendDisputeWindow` and `submitResult` (T413)
- **Custom errors**: Converted all `require()` string reverts to gas-efficient custom errors in `TEEMLVerifier`, `ProverReputation`, `Upgradeable`, and `ProgramRegistry` (Phases 75-77)
- **Guardian override**: Added `guardianOverride` modifier to `ProgramRegistry.updateProgramUrl` for emergency admin control (T423)
- **Code-length validation**: `ProgramRegistry.updateVerifier` now validates the verifier address has code deployed (T399)
- **Hex validation**: Indexer now validates `model_hash` and `after_id` parameters for proper hex format (T416)
- **SSRF protection**: Prover security hardening with SSRF protection and auth improvements (T397/T403/T409)
- **Hardened Upgradeable**: Strengthened `reinitialize` and `_authorizeUpgrade` in UUPS proxy (Phase 74)
- **Critical unwrap fixes**: Fixed `unwrap()` panics, mutex poisoning, and error swallowing in Rust prover (Phase 74)

### Gas Optimizations

- **Cached storage refs**: Cache storage references in `RemainderVerifier` circuit management functions to reduce redundant SLOADs (T420)
- **Custom errors save gas**: Custom errors use 4-byte selectors instead of storing revert strings, saving ~200 gas per revert (Phases 75-76)
- **DAG deactivation**: Added `deactivateDAGCircuit` for circuit lifecycle management (Phase 75)

### Documentation

- **NatSpec annotations**: Added `@param` and `@return` NatSpec comments to `RemainderVerifier` DAG functions (T421)
- **NatSpec across contracts**: Phase 77 added comprehensive NatSpec to security-hardened contracts
- **SDK quickstart**: Updated quickstart examples to use SDK classes (T396)
- **Interface updates**: Fixed dead config fields and updated contract interfaces (Phase 71)

### Testing

- **String length bounds tests**: Added bounds checking tests for `ProgramRegistry` and `ProverRegistry` string fields (T418)
- **Fuzz tests**: Added fuzz tests for tip decay, fee splits, slash math, and cooldowns
- **Operator unit tests**: Comprehensive unit tests for operator `chain.rs`
- **ProverRegistry fuzz tests**: Added fuzz tests and storage layout safety tests (T388/T404)
- **Ownable2Step tests**: Added tests verifying two-step ownership transfer behavior

### CI/CD

- **Fixture freshness check**: CI now validates test fixtures are up-to-date (T424)
- **Deduplicated CI jobs**: Removed redundant TypeScript SDK, lint, and build jobs (Phase 76)
- **World Chain deploy workflow**: Added deployment workflow for World Chain (Phase 70)
- **Rust CI caching**: Improved Rust build caching in CI (Phase 70)

### Infrastructure

- **Operator improvements**: LRU eviction for caches, config validation, admin API warning headers (T394/T395/T408)
- **Python SDK fix**: Fixed silent exception swallowing in Python SDK (T392)
- **Indexer hardening**: Circuit breaker logging improvements (Phase 77)
- **Helm chart**: Added HPA and PDB for prover and indexer (T405)
- **Lint cleanup**: Suppressed remaining clippy warnings and fixed `map_or` (Phase 73)
- **Reputation tracking**: Added `reportBadProof` function for missed claim deadline tracking (T387)

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
