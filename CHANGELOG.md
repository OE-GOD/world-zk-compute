# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-03-26

Initial release. Cryptographic proof that ML models produce correct outputs.

### Added

#### Verifier Core (`crates/zkml-verifier/`)
- Chain-agnostic `zkml-verifier` Rust crate for GKR+Hyrax proof verification
- C-FFI bindings (`ffi.rs`) for Python, Go, and Java integration
- WASM compilation target (`wasm.rs`) for browser-based verification
- Python wrapper package (`pip install zkml-verifier`)
- npm package (`@worldzk/verifier`) for JavaScript and TypeScript
- JSON proof bundle format with metadata and hex encoding
- Gzip compression for proof bundles with auto-detection
- CLI binary (`zkml-verifier verify <bundle.json>`)

#### Supported Models
- XGBoost tree ensemble inference (native JSON parser)
- LightGBM tree ensemble inference
- Random forest inference (scikit-learn JSON format)
- Logistic regression inference (48-bit decomposition)
- Model format auto-detection from JSON structure

#### XGBoost ZK Circuit (`examples/xgboost-remainder/`)
- GKR+Hyrax zero-knowledge circuit for XGBoost tree inference
- Phase 1a: leaf selection and aggregation via fold-based reduction
- Phase 1b: comparison verification with bit decomposition (K=18)
- 88-layer DAG circuit for the sample credit scoring model
- gnark Groth16 wrapper for EC equation checks (`gnark-wrapper/`)
- Chunked EC Groth16 proving for large circuits (500 ops/chunk)
- Rust ABI encoder for on-chain proof submission

#### Smart Contracts (`contracts/`)
- **RemainderVerifier** with UUPS proxy pattern (upgradeable)
- **RemainderVerifierUpgradeable** with constructor initialization guard
- **TEEMLVerifier** with UUPS proxy (TEE attestation + ZK dispute resolution)
- **GKRVerifier** for single-circuit GKR+Hyrax proof verification
- **GKRDAGVerifier** for arbitrary DAG circuits (multi-layer, multi-claim RLC)
- **GKRHybridVerifier** for Groth16-wrapped EC checks (single circuit)
- **GKRDAGHybridVerifier** for DAG Groth16-wrapped EC checks
- **DAGBatchVerifier** for multi-transaction verification (8 layers/batch, <30M gas/tx)
- **HybridStylusGroth16Verifier** for Stylus WASM + Groth16 hybrid path
- **DAGRemainderGroth16Verifier** (gnark-exported Groth16 verifier with embedded VK)
- **RemainderGroth16Verifier** (gnark-exported, single-circuit variant)
- **PoseidonSponge** (width=3, rate=2, BN254 scalar field, PSE/Scroll constants)
- **SumcheckVerifier** and **CommittedSumcheckVerifier**
- **HyraxVerifier** and **HyraxProofDecoder**
- **ExecutionEngine** with request lifecycle and tip escalation
- **ProgramRegistry** (permissionless registration, admin verification)
- **ProverRegistry** with Ownable2Step
- **ProverReputation** with decay-based scoring
- **TimelockController** for admin operations
- **Pausable** emergency mechanism on all entry points
- Implementation contract initialization protection (`_initialized = type(uint8).max`)
- Zero-address validation on critical initializer parameters
- Checks-effects-interactions pattern in ExecutionEngine
- Custom errors across all contracts (no revert strings)
- NatSpec documentation on public and external functions

#### Stylus WASM Verifier (`contracts/stylus/gkr-verifier/`)
- Rust/WASM port of GKR DAG verifier for Arbitrum Stylus
- 52KB raw / 24.5KB Brotli (under 24KB Stylus deployment limit)
- BN254 field arithmetic, EC operations via EVM precompiles
- Poseidon sponge, GKR compute layers, Hyrax input layers, sumcheck
- Hybrid mode: transcript replay + Fr arithmetic only (feature-gated)
- Pure Rust BN254 EC for native testing; Stylus `RawCall` for WASM
- Knuth Algorithm D for 512-bit modular reduction

#### REST API (`services/verifier/`)
- Standalone verifier service (`POST /verify`, `/verify/batch`, `/verify/hybrid`)
- API key authentication with `X-API-Key` header
- Token bucket rate limiting per client
- Multi-tenant API key management with admin API
- Per-tenant rate limit configuration
- Circuit registration TTL expiry
- TLS and mTLS support via rustls
- OpenAPI 3.0.3 specification

#### Operator Service (`services/operator/`)
- Submit, watch, and auto-dispute workflow
- Multi-model support with `find_model_by_hash`
- Proof archive with expiry and rotation
- Alerting, audit logging, and metrics
- Circuit breaker and deadline monitoring
- SSRF protection for external calls

#### TEE Enclave (`tee/`)
- AWS Nitro enclave for XGBoost and LightGBM inference
- ECDSA attestation with EIP-712 typed data signing
- Enclave image hash (PCR0) recorded on-chain at registration
- Nitro enclave Dockerfile and EIF build script
- Key rotation script (`rotate-enclave-key.sh`)

#### Additional Services
- **Indexer** (`services/indexer/`) with PostgreSQL storage, REST API, WebSocket, and rate limiting
- **Health check aggregator** (`services/health-aggregator/`)
- **Admin CLI** (`services/admin-cli/`) for contract and service management

#### SDKs and Examples
- Python SDK quickstart (`examples/sdk-python-quickstart/`)
- Rust SDK quickstart (`examples/sdk-rust-quickstart/`)
- TypeScript SDK quickstart (`examples/sdk-typescript-quickstart/`)
- Bank credit scoring demo (`examples/bank-demo/`)
- Browser WASM demo (`web/`)

#### Deployment
- Docker Compose for bank demo, production, monitoring, Sepolia, GPU, and load testing
- fly.io deployment config for verifier service
- Arbitrum mainnet deployment script with safety checks (`scripts/deploy-mainnet.sh`)
- Arbitrum Sepolia testnet deployment script (`scripts/deploy-sepolia.sh`)
- World Chain mainnet deployment script (`contracts/script/DeployWorldChainMainnet.s.sol`)
- Stylus WASM deployment script (`scripts/stylus-sepolia-deploy.sh`)
- Nitro enclave EIF build script (`tee/scripts/build-enclave.sh`)
- systemd-compatible operator service configuration

#### CI/CD (`.github/workflows/`)
- Rust CI (cargo check, clippy, test, fmt)
- Solidity CI (forge build, test, fmt)
- Stylus CI (WASM build, size check, native tests)
- Python SDK CI
- E2E smoke tests (local Anvil and Sepolia)
- Docker security scanning
- Coverage reporting
- PyPI and npm publish workflows
- Release workflow with artifact generation

#### Documentation (`docs/`)
- Security model and threat analysis (`SECURITY_MODEL.md`)
- Audit trail format for compliance (`AUDIT_TRAIL.md`)
- Static analysis findings with fixes (`AUDIT_FINDINGS.md`)
- API reference for CLI, REST, Rust, Python, C FFI (`API_REFERENCE.md`)
- Quick start tutorial -- Python, Rust, JS, REST (`QUICKSTART.md`)
- Architecture overview (`ARCHITECTURE.md`)
- TEE deployment guide (`TEE_DEPLOYMENT.md`)
- Contract deployment guide (`CONTRACT_DEPLOYMENT.md`)
- Stylus deployment guide (`STYLUS_DEPLOYMENT.md`)
- Monitoring guide with Grafana dashboard (`MONITORING.md`)
- Publishing guide for PyPI, npm, crates.io (`PUBLISHING.md`)
- Gas optimization notes (`GAS_OPTIMIZATION.md`)
- Disaster recovery runbook (`DISASTER_RECOVERY.md`)
- Troubleshooting guide (`TROUBLESHOOTING.md`)

#### Security
- Slither static analysis (2 high, 5 medium findings -- 4 fixed, 3 accepted as design decisions)
- Fuzz testing: 3 Foundry fuzz contracts + 1 invariant contract (`ExecutionEngineFuzz`, `ProverRegistryFuzz`, `TEEMLVerifierFuzz`, `InvariantTest`)
- Implementation contract initialization protection on all UUPS proxies
- Zero-address validation on TEEMLVerifier initializer
- Checks-effects-interactions pattern enforced in ExecutionEngine
- `nonReentrant` on all state-changing external functions
- No `tx.origin` or `selfdestruct` usage
- `gitleaks` pre-commit hook for secret detection

#### Performance
- Warm prover cache (cached Pedersen generators)
- Verification benchmarks (Criterion, 88-layer XGBoost circuit):
  - Hybrid (Fr only): 629ms
  - Full (`verify_raw`): 2.16s
  - Full with JSON bundle (`verify`): 6.29s
- Proof size: 130.7KB binary, 432.9KB JSON bundle
- Gzip compression reduces bundle size approximately 70%
- On-chain gas costs:
  - Stylus + Groth16 hybrid: ~3-6M gas (single tx, Arbitrum)
  - Multi-tx batch: <30M gas per tx, 15 transactions total (Ethereum L1)
  - Direct GKR+Hyrax: ~254M gas (high-gas-limit chains)

#### Test Coverage
- 1,570+ tests across all components
- Solidity: 789 tests (verifier, batch, Groth16 hybrid, TEE, fuzz)
- Python SDK: 449 tests
- TypeScript SDK: 199 tests
- Stylus WASM: 85 tests (BN254 field/EC, Poseidon, GKR, Hyrax, sumcheck)
- Remainder circuit: 34 tests (XGBoost circuit, prove-and-verify, ABI encoding)
- gnark Go: 14 tests (Groth16 circuit, proving, chunked EC)

[0.1.0]: https://github.com/worldcoin/world-zk-compute/releases/tag/v0.1.0
