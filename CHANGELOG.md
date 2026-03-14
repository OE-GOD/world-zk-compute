# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Contracts**: TEEMLVerifier, ExecutionEngine, ProgramRegistry with upgrade support (UUPS proxy)
- **Contracts**: RemainderVerifier — GKR/Hyrax on-chain verifier for XGBoost circuits
- **Contracts**: GKRDAGVerifier — arbitrary DAG circuit verification
- **Contracts**: DAGBatchVerifier — multi-tx batch verification (15 txs for 88-layer circuits)
- **Contracts**: Groth16 hybrid verification path for both linear and DAG circuits
- **Contracts**: Stylus WASM port of GKR DAG verifier for Arbitrum (24.5KB compressed)
- **TEE Enclave**: AWS Nitro attestation with P-384 cert chain verification
- **TEE Enclave**: Replay protection (nonce dedup + timestamp freshness)
- **TEE Enclave**: Model registry with XGBoost + LightGBM support
- **TEE Enclave**: Watchdog, metrics, validation, and graceful shutdown
- **Operator Service**: Event watcher, auto-dispute, proof submission
- **Operator Service**: Circuit breaker, rate limiting, deadline monitor
- **Operator Service**: TOML config, multi-contract watching, webhook notifications
- **Operator Service**: Metrics (Prometheus), tracing (OpenTelemetry-ready)
- **Indexer Service**: REST + WebSocket API for on-chain event indexing
- **Admin CLI**: Contract management tool (pause, unpause, register, revoke, status)
- **Shared Crates**: `tee-watcher` (event filtering), `tee-events` (ABI types)
- **Rust SDK**: Client, TEE verifier, event watcher, hash utilities, retry logic
- **Python SDK**: TEEVerifier, XGBoostConverter, LightGBMConverter, EventWatcher, BatchVerifier, async client, CLI
- **TypeScript SDK**: TEEVerifier, TEEEventWatcher, BatchVerifier, hash utilities
- **XGBoost Circuit**: Full inference circuit with leaf selection + comparison verification (Phase 1a+1b)
- **XGBoost Circuit**: DAG verifier integration (Phase 1c) with 88-layer compute
- **Prover**: risc0-zkvm v3.0 warm prover with HTTP API
- **Docker**: Full-stack compose (anvil + deployer + enclave + prover + operator)
- **Docker**: GPU compose, Sepolia compose, monitoring compose, load test compose, test compose
- **Helm**: Kubernetes deployment templates (operator, indexer, enclave, prover, private-input)
- **Helm**: HPA, PDB, NetworkPolicy, canary deployment support
- **CI**: Rust, contracts, SDK, Stylus, security audit, benchmarks, E2E workflows
- **Scripts**: Deployment (local, Sepolia, multichain), E2E testing, load testing
- **Scripts**: Sepolia operations (register, submit, query, dispute, events, status)
- **Scripts**: Operational tooling (health check, emergency pause, audit, env generation, docker logs)
- **Scripts**: CI preflight, SDK E2E testing, contract size checking
- **Monitoring**: Prometheus scrape config, Grafana dashboards (operator, enclave, indexer, Sepolia)
- **Monitoring**: Alerting rules (operator down, high dispute rate, enclave errors)
- **Docs**: Architecture, threat model, upgrade guide, runbook, troubleshooting, gas optimization
- **Docs**: SDK quickstart, Sepolia quickstart, disaster recovery, performance SLOs
- **Private Input Server**: Minimal Python HTTP server for storing execution inputs
- **Security**: SECURITY.md, GitHub issue templates, PR template
