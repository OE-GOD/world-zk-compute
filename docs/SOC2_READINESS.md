# SOC2 Readiness Assessment

**Last updated:** 2026-11-23
**Scope:** World ZK Compute off-chain and on-chain verification platform
**Audience:** Compliance officers, auditors, SOC2 assessors
**Status:** Pre-audit readiness assessment

---

## Executive Summary

World ZK Compute provides verifiable ML inference with cryptographic audit
trails. This document maps existing security controls to SOC2 Trust Service
Criteria (TSC) and identifies gaps requiring remediation before a Type II
audit engagement.

**Overall readiness:** of the 64 controls in the tables below, 39 are marked
implemented (one of them as the core feature), 20 partially implemented, 4 not
implemented and 1 not tested. 8 gaps requiring remediation are listed at the end.

---

## Security (CC6: Logical and Physical Access Controls)

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| API key authentication | CC6.1 | Gateway API keys (`GATEWAY_API_KEYS`), auth middleware in the gateway, verifier and proof registry (`auth_middleware` in gateway/src/main.rs, verifier/src/auth.rs and proof-registry/src/routes.rs); each is off unless its keys are set, and the proof generator and the enclave's `/infer` have none | Partially implemented |
| Multi-tenant access control | CC6.1 | Per-tenant API keys in the verifier service; the proof registry has no tenant concept, so stored proofs are not isolated per tenant | Partially implemented |
| Encryption in transit | CC6.1 | Optional TLS and mTLS in the verifier service via rustls (`VERIFIER_TLS_CERT`, `VERIFIER_TLS_KEY`, `VERIFIER_TLS_CLIENT_CA`); the gateway has no TLS support | Partially implemented |
| Encryption at rest | CC6.1 | SOPS + age for secrets encryption and KMS integration are documented in `docs/SECRETS.md`, but the repository has no `.sops.yaml` and no encrypted env file; the Terraform proof bucket sets KMS server-side encryption | Partially implemented |
| Key management | CC6.1 | Rotation scripts (`rotate-keys.sh`, `rotate-enclave-key.sh`); SOPS + age key encryption and AWS KMS integration are documented but not configured in the repository. See `docs/SECRETS.md` | Partially implemented |
| Key rotation procedures | CC6.1 | Documented for all secret tiers (API keys, operator key, enclave key, admin key). Quarterly schedule for API keys, annual for operator keys | Implemented |
| Structured audit logging | CC6.2 | `tracing` logs with `audit` target in the operator, enclave, gateway and indexer; the proof registry has no `audit` target, and the enclave logs plain text, not JSON | Partially implemented |
| Vulnerability management | CC6.6 | Slither static analysis on Solidity contracts, `cargo audit` for Rust dependencies, fuzz tests for TEEMLVerifier, ExecutionEngine and ProverRegistry and four cargo-fuzz targets for the zkml-verifier crate (none for the Poseidon sponge) | Implemented |
| Rate limiting | CC6.6 | Configurable rate limiting on the enclave's `/infer` (`MAX_REQUESTS_PER_MINUTE`), PID limits on Docker containers | Implemented |
| SSRF protection | CC6.6 | Input validation on URLs, network segmentation (frontend/backend Docker networks) | Implemented |
| Container hardening | CC6.6 | `read_only` filesystem, `no-new-privileges`, `cap_drop: ALL`, PID limits, memory limits: all five on 15 of the 41 services in the 10 root-level compose files; `docker-compose.bank-demo.yml` and `docker-compose.test.yml` set none of them | Partially implemented |
| Secret zero-exposure | CC6.1 | `SecretString` type for private keys (prevents logging, zeroizes on drop). `sops exec-env` for zero-disk-exposure decryption is documented only; SOPS is not set up in the repository | Partially implemented |

---

## Availability (A1: System Availability)

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Health monitoring | A1.1 | `/health` endpoints on all services, `/ready` on the operator and indexer only; Prometheus scrape at 15s intervals | Partially implemented |
| Metrics collection | A1.1 | Prometheus metrics: `operator_active_disputes`, `operator_uptime_seconds`, `operator_last_block_polled`, etc. See `docs/MONITORING.md` | Implemented |
| Alerting rules | A1.1 | Prometheus alerting rules for operator down, dispute rate, operator errors, failed disputes and webhook failures; there is no block-polling-stall rule, and the four enclave rules depend on scraping the enclave's `/metrics`, which is JSON, not Prometheus format. See `monitoring/alerting-rules.yml` | Partially implemented |
| Webhook notifications | A1.1 | Slack-compatible webhooks for dispute events, proof submissions, resolution outcomes | Implemented |
| Dashboard | A1.1 | Grafana dashboard (`deploy/grafana-dashboard.json`) with 7 panel rows: health, throughput, disputes, errors, gas costs | Implemented |
| Backup procedures | A1.2 | The repository has no scheduled backup job, no S3 upload, and no operator-state or SQLite online backup; the only backup tools are the manual scripts `scripts/backup.sh` and `scripts/backup-deployment.sh` (restore: `scripts/restore.sh`). See `docs/DISASTER_RECOVERY.md` | Not implemented |
| Disaster recovery plan | A1.2 | `docs/DISASTER_RECOVERY.md` documents RPO/RTO for the verifier API, gateway, proof registry and proof generator only; it has no figures for the operator, indexer, enclave or full stack | Partially implemented |
| Alertmanager / PagerDuty | A1.1 | Alertmanager configuration documented; PagerDuty/Slack receiver templates provided | Partially implemented |
| DR drill schedule | A1.2 | DR plan documented but quarterly drill not yet conducted | Not tested |
| Multi-region failover | A1.3 | Single-region deployment. Neither RPC endpoint failover nor multi-region is documented or configured | Not implemented |

---

## Processing Integrity (PI1: System Processing)

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Cryptographic verification | PI1.1 | GKR+Hyrax ZK proofs guarantee computation integrity with ~2^-244 soundness error (88-layer XGBoost). See `docs/SECURITY_MODEL.md` Section 5.1 | **Core feature** |
| Proof completeness | PI1.1 | Self-contained `ProofBundle` JSON format: proof, generators, circuit description, model hash. Anyone can verify independently | Implemented |
| Multiple verification paths | PI1.1 | 5 paths: Rust CLI, Python SDK, JavaScript/WASM, REST API, on-chain Solidity. All implement same protocol | Implemented |
| Tamper detection | PI1.2 | Merkle-tree transparency log (Certificate Transparency model) for all proof submissions. SHA-256 proof hashes, Merkle inclusion proofs | Implemented |
| Signed verification receipts | PI1.2 | ECDSA-signed `VerificationReceipt` with proof_id, circuit_hash, model_hash, timestamp, verifier version, host | Implemented |
| Proof archive immutability | PI1.2 | Append-only date-partitioned directory structure with atomic writes (write-to-tmp, rename). The proof registry has S3 Object Lock code behind the cargo feature `s3` (off by default, not built into the Docker image, and no test runs it against S3); `deploy/terraform/main.tf` declares Object Lock in GOVERNANCE mode on the proof bucket | Partially implemented |
| Challenge-dispute mechanism | PI1.3 | 1-hour challenge window + 24-hour dispute resolution. Economic bonding (0.1 ETH stake/bond). Timeout fallback | Implemented |
| Input commitment binding | PI1.1 | Pedersen commitments bind input features to proofs. Computationally hiding under discrete log assumption on BN254 | Implemented |
| Model version binding | PI1.1 | `modelHash = keccak256(model_file_bytes)` binds proofs to specific model versions. Circuit hash binds to model topology | Implemented |

---

## Confidentiality (C1: Confidential Information Protection)

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Data classification | C1.1 | Proofs contain hashes and commitments, not raw data. Model weights are confidential (never on-chain). Input features protected by Pedersen commitments. See `docs/SECURITY_MODEL.md` Section 5.3 | Implemented |
| On-chain data minimization | C1.1 | Only `inputHash` (keccak256 digest) stored on-chain. Raw features only in access-controlled off-chain archives | Implemented |
| Access control on proof archives | C1.2 | Tiered access model: public (on-chain events), low (proof bundles without features), high (proof bundles with features -- compliance/legal only). It is a recommendation table in `docs/AUDIT_TRAIL.md` Section 4.5; no service enforces the tiers | Partially implemented |
| Secret zeroization | C1.2 | `SecretString` wraps private keys; `zeroize` on drop prevents memory exposure | Implemented |
| Secret tier classification | C1.1 | 4-tier classification: Critical (admin key), High (operator/enclave keys), Medium (registry signing key, RPC keys), Low (API keys, monitoring passwords). See `docs/SECRETS.md` | Implemented |

---

## CC1: Control Environment

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Code of conduct | CC1.1 | Apache-2.0 license, CONTRIBUTING.md | Implemented |
| Code ownership | CC1.2 | `.github/CODEOWNERS` (one owner for every path); there is no `check-domain.sh` or other domain enforcement | Partially implemented |
| Role separation | CC1.3 | Multi-agent coordination with domain boundaries (builder-a: Rust, builder-b: Solidity, researcher: docs). File-level locking | Implemented |

---

## CC2: Communication and Information

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Structured logging | CC2.1 | JSON-structured `tracing` output in the gateway, verifier, proof registry, proof generator, indexer and (with `LOG_FORMAT=json`) the operator; the enclave, the risc0 prover and the private input server log plain text. `RUST_LOG=audit=info` for audit-specific routing to SIEM | Partially implemented |
| API documentation | CC2.2 | `docs/API_REFERENCE.md`, `docs/QUICKSTART.md`, `docs/SDK_QUICKSTART.md`, OpenAPI specs (`services/verifier/openapi.yaml`, `services/indexer/openapi.yaml`, `services/operator/openapi.yaml`) | Implemented |
| Incident communication | CC2.3 | No templates for service degradation, resolution, or emergency pause notifications exist in the repository; `docs/DISASTER_RECOVERY.md` has no Section 7 | Not implemented |

---

## CC3: Risk Assessment

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Threat modeling | CC3.1 | `docs/THREAT_MODEL.md` -- 9 attack vectors with mitigations. `docs/FRONT_RUNNING.md` -- MEV analysis per function | Implemented |
| Security model documentation | CC3.1 | `docs/SECURITY_MODEL.md` -- trust model, cryptographic assumptions, known limitations, verification paths | Implemented |
| DAG batch security analysis | CC3.2 | `docs/DAG_BATCH_SECURITY.md` -- multi-transaction security properties, session auth, ordering guarantees | Implemented |
| Static analysis findings | CC3.2 | `docs/AUDIT_FINDINGS.md` -- Slither results + manual review. H-1 (unprotected init) fixed, 5 medium findings tracked | Implemented |
| Gas cost profiling | CC3.2 | `docs/BENCHMARKS.md`, `docs/GAS_OPTIMIZATION.md` -- per-operation gas measurements | Implemented |

---

## CC5: Control Activities

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Defense-in-depth | CC5.1 | Three-tier: TEE hardware isolation, economic bonding (stake/bond), ZK mathematical proof | Implemented |
| Cryptographic controls | CC5.2 | ECDSA (secp256k1), Poseidon hash (BN254), GKR+Hyrax proofs, Groth16 SNARKs. See `docs/SECURITY_MODEL.md` Section 4 | Implemented |
| Emergency pause | CC5.3 | `Pausable` on all core contracts (TEEMLVerifier, ExecutionEngine, RemainderVerifier). Admin-only `pause()`/`unpause()` | Implemented |
| Reentrancy protection | CC5.2 | `ReentrancyGuard` (OpenZeppelin) on all ETH-transferring functions. Checks-effects-interactions pattern | Implemented |
| Access control (contracts) | CC5.3 | `Ownable2Step` on ExecutionEngine, ProgramRegistry, ProverRegistry and ProverReputation; TEEMLVerifier and RemainderVerifier have a one-step `changeAdmin(address)` instead. Stake caps at 100 ETH | Partially implemented |
| UUPS proxy upgradeability | CC5.3 | `RemainderVerifier` uses UUPS proxy; upgrades are guarded by `onlyTimelocked` (the admin while no timelock is set). Implementation init disabled via constructor | Implemented |

---

## CC7: System Operations

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Infrastructure-as-code | CC7.1 | Docker Compose (10 files at the repository root), Helm charts, Kustomize overlays, Terraform (deploy/) | Implemented |
| CI/CD pipeline | CC7.2 | GitHub Actions workflows for Rust tests, Solidity tests, Slither analysis, deployment | Implemented |
| Incident management | CC7.3 | Health aggregator, Prometheus alerting, Slack webhooks; there are no incident templates. See `docs/RUNBOOK.md` | Partially implemented |
| Backup and recovery | CC7.4 | Proof archive with date partitioning and atomic writes; S3 Object Lock only as registry code behind the cargo feature `s3` (off by default), and backup is a manual script. See `docs/DISASTER_RECOVERY.md` | Partially implemented |
| Escalation procedures | CC7.3 | 4-tier severity (P0-P3) with response times: P0 <15 min, P1 <1 hour, P2 <4 hours, P3 next business day | Implemented |

---

## CC8: Change Management

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Change authorization | CC8.1 | `.github/CODEOWNERS` exists, but `main` has no branch protection or ruleset, so PR reviews are not required | Not implemented |
| Automated testing | CC8.2 | 166+ Solidity tests, 75+ Stylus WASM tests, 34+ Rust tests, 60+ Go tests, CI on every PR | Implemented |
| Deployment automation | CC8.3 | Docker Compose for dev, Helm for Kubernetes, deployment scripts with dry-run mode (`--dry-run` on `deploy.sh`, `deploy-mainnet.sh` and `deploy-multichain.sh`) | Implemented |

---

## CC9: Risk Mitigation

| Control | TSC Ref | Evidence | Status |
|---------|---------|----------|--------|
| Economic security (bonding) | CC9.1 | Prover stake (0.1 ETH) + challenger bond (0.1 ETH). Winner takes both. Configurable with 100 ETH cap | Implemented |
| Dependency pinning | CC9.2 | `Cargo.lock` committed, exact version pins in `Cargo.toml`, `cargo audit` in CI | Implemented |
| Contract upgrade safety | CC9.1 | UUPS proxy with a one-step `changeAdmin(address)` admin (not `Ownable2Step`). Timelock recommended for production (documented, not deployed) | Partially implemented |

---

## Gaps and Remediation Plan

| # | Gap | TSC Ref | Priority | Remediation | Target Date |
|---|-----|---------|----------|-------------|-------------|
| G-1 | **No formal third-party security audit** | CC3, CC5 | P0 | Engage Trail of Bits, OpenZeppelin, or equivalent for audit of TEEMLVerifier.sol, RemainderVerifier.sol, operator service, and zkml-verifier crate | Before mainnet |
| G-2 | **Single admin key for contracts** | CC6.1 | P0 | Deploy Gnosis Safe multisig as contract owner. Add timelock for `setRemainderVerifier()`. Document signer policy | Before mainnet |
| G-3 | **EIP-712 attestation field mismatch** | CC6.1, PI1.2 | P0 | Update `TEEMLVerifier.submitResult()` to verify extended attestation fields (chainId, nonce, timestamp) on-chain. See `docs/SECURITY_MODEL.md` Section 6.7 | Before mainnet |
| G-4 | **No formal penetration test** | CC3.2 | P1 | Commission external pentest against deployed infrastructure (API gateway, verifier, registry, enclave) | Q2 2026 |
| G-5 | **DR drill not conducted** | A1.2 | P1 | Schedule quarterly DR drill: simulate operator failure, enclave compromise, and full stack recovery | Q2 2026 |
| G-6 | **SIEM integration not deployed** | CC2.1, CC7.3 | P1 | Configure structured log forwarding from operator/enclave/registry to Datadog, Splunk, or Grafana Loki | Q2 2026 |
| G-7 | **Groth16 trusted setup ceremony** | CC5.2 | P2 | Conduct multi-party ceremony (Powers of Tau) for EC circuit Groth16 wrapper. Current setup is development-only | Q3 2026 |
| G-8 | **Timelock not deployed on admin actions** | CC5.3, CC9.1 | P1 | Deploy OpenZeppelin `TimelockController` for `setRemainderVerifier()`, `setProverStake()`, and `setChallengeBondAmount()` | Before mainnet |

---

## Regulatory Alignment

### ECOA / FCRA (US Credit Decisions)

- **Model provenance:** `modelHash` provides cryptographic binding to specific model version
- **Decision reproducibility:** ZK proof guarantees output was produced by declared model on declared inputs
- **Audit trail immutability:** On-chain records + off-chain proof archives with S3 Object Lock
- **7-year retention:** Configurable archive retention per Regulation B (12 CFR 1002.12)

### EU AI Act (High-Risk AI Systems)

- **Art. 11 (Technical documentation):** Circuit description, model hash, proof bundle
- **Art. 12 (Record-keeping):** Timestamped, tamper-evident proof archives + on-chain events
- **Art. 13 (Transparency):** Circuit structure registered on-chain; model hash provides version traceability
- **Art. 14 (Human oversight):** Challenge mechanism provides human-in-the-loop dispute path
- **Art. 15 (Accuracy):** ZK proof mathematically guarantees computation accuracy

### GDPR

- **Data minimization:** Only `inputHash` on-chain; raw features in access-controlled off-chain archives
- **Pedersen commitment hiding:** Input features protected under discrete log assumption on BN254
- **Right to erasure:** Off-chain archives containing features can be purged; on-chain hashes remain (non-personal data)

See `docs/AUDIT_TRAIL.md` Section 3 for detailed compliance analysis.

---

## Evidence Index

| Document | Location | Relevance |
|----------|----------|-----------|
| Security model | `docs/SECURITY_MODEL.md` | Trust model, cryptographic assumptions, known limitations |
| Threat model | `docs/THREAT_MODEL.md` | Attack vectors and mitigations |
| Audit trail | `docs/AUDIT_TRAIL.md` | Audit artifacts, verification process, compliance |
| Audit findings | `docs/AUDIT_FINDINGS.md` | Static analysis results and manual review |
| Disaster recovery | `docs/DISASTER_RECOVERY.md` | RPO/RTO, backup procedures, failover |
| Secrets management | `docs/SECRETS.md` | Key management, rotation, revocation |
| Monitoring | `docs/MONITORING.md` | Prometheus metrics, Grafana dashboards, alerting |
| Production readiness | `docs/PRODUCTION_READINESS.md` | Deployment checklist, operator config, incident response |
| Front-running analysis | `docs/FRONT_RUNNING.md` | MEV analysis per contract function |
| DAG batch security | `docs/DAG_BATCH_SECURITY.md` | Multi-tx verification security properties |
| API reference | `docs/API_REFERENCE.md` | Full REST API documentation |
| Architecture | `docs/ARCHITECTURE.md` | System design, data flow, component descriptions |
