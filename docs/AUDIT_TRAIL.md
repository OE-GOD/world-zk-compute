# Audit Trail: World ZK Compute

**Last updated:** 2026-03-26
**Version:** 1.0
**Scope:** Inference result lifecycle, proof bundles, on-chain events, off-chain archives
**Audience:** Compliance teams, security auditors, integration engineers

---

## 1. What Is Auditable

Every ML inference processed by World ZK Compute produces a verifiable record. The system generates audit artifacts at three levels: on-chain state, off-chain proof bundles, and operator audit logs.

### 1.1 On-Chain Record (TEEMLVerifier)

Each submitted inference result creates an immutable on-chain record containing:

| Field | Type | Description |
|---|---|---|
| `resultId` | `bytes32` | Unique identifier: `keccak256(submitter, modelHash, inputHash, blockNumber)` |
| `modelHash` | `bytes32` | keccak256 hash of the serialized model weights |
| `inputHash` | `bytes32` | keccak256 hash of the input feature vector (privacy-preserving) |
| `resultHash` | `bytes32` | keccak256 hash of the raw inference output bytes |
| `result` | `bytes` | Raw inference output (e.g., JSON-encoded prediction scores) |
| `enclave` | `address` | Ethereum address of the TEE enclave that signed the attestation |
| `submitter` | `address` | Address that submitted the result on-chain |
| `submittedAt` | `uint40` | Block timestamp of submission |
| `challengeDeadline` | `uint40` | Deadline for challenges (submission + challengeWindow) |
| `finalized` | `bool` | Whether the result has been finalized |
| `challenged` | `bool` | Whether the result was challenged |
| `challenger` | `address` | Address that filed the challenge (zero if unchallenged) |
| `proverStakeAmount` | `uint256` | ETH staked by the submitter |
| `challengeBond` | `uint256` | ETH posted by the challenger |

### 1.2 On-Chain Events

The following events are emitted and indexed for querying:

| Event | When | Indexed Fields |
|---|---|---|
| `EnclaveRegistered(enclaveKey, enclaveImageHash)` | New enclave registered | `enclaveKey` |
| `EnclaveRevoked(enclaveKey)` | Enclave deactivated | `enclaveKey` |
| `ResultSubmitted(resultId, modelHash, inputHash, submitter)` | Inference result submitted | `resultId`, `modelHash`, `submitter` |
| `ResultChallenged(resultId, challenger)` | Result challenged | `resultId` |
| `DisputeResolved(resultId, proofValid)` | ZK proof verified or timeout | `resultId` |
| `DisputeExtended(resultId, newDeadline)` | Dispute window extended | `resultId` |
| `ResultFinalized(resultId)` | Result finalized (unchallenged) | `resultId` |
| `ProofVerified(circuitHash, valid)` | RemainderVerifier proof check | `circuitHash` |
| `DAGProofVerified(circuitHash, valid, method)` | DAG proof verification | `circuitHash` |

### 1.3 Off-Chain Proof Bundle (ProofArchiveEntry)

The operator service archives a `ProofArchiveEntry` for each inference in a date-partitioned directory (`$PROOFS_DIR/YYYY-MM-DD/proof-{uuid}.json`). Each entry contains:

| Field | Type | Description |
|---|---|---|
| `id` | `string` | UUID for this archive entry |
| `archived_at` | `string` | ISO-8601 timestamp |
| `result_id` | `string` | On-chain result ID (hex) |
| `model_hash` | `string` | Model hash (hex) |
| `input_hash` | `string` | Input hash (hex) |
| `result_hash` | `string` | Result hash (hex) |
| `result` | `string` | Raw inference result (hex) |
| `attestation` | `string` | ECDSA attestation signature (hex, 65 bytes) |
| `features` | `f64[]` | Input feature values |
| `chain_id` | `u64` | Chain ID |
| `enclave_address` | `string` | Enclave signer address |
| `proof_path` | `string?` | Path to ZK proof file (if generated) |
| `disputed` | `bool` | Whether the result was disputed |
| `finalized` | `bool` | Whether the result was finalized |

### 1.4 ZK Proof Bundle (ProofBundle)

When a ZK proof is generated (either preemptively or in response to a challenge), the full `ProofBundle` contains:

| Field | Type | Description |
|---|---|---|
| `proof_hex` | `string` | ABI-encoded GKR+Hyrax proof (with "REM1" selector prefix) |
| `public_inputs_hex` | `string` | Public input values (hex) |
| `gens_hex` | `string` | Pedersen generators (hex, ~33KB for 512 generators) |
| `dag_circuit_description` | `JSON` | Circuit topology (layer types, connectivity, oracle expressions) |

This bundle is self-contained: anyone with the bundle can independently verify the proof using the `zkml-verifier` crate, the verifier REST service, or the on-chain contracts.

### 1.5 Operator Audit Logs

The operator service emits structured audit events via the `tracing` framework with target `audit`. These can be routed to a SIEM system via `RUST_LOG=audit=info`. Events include:

| Event | Fields |
|---|---|
| `enclave_registered` | `enclave_address`, `image_hash`, `tx_hash`, `operator` |
| `enclave_registration_attempted` | `enclave_address`, `pcr0`, `skip_verify` |
| `result_submitted` | `tx_hash`, `model_name`, `feature_count` |
| `challenge_detected` | `result_id`, `challenger`, `block_number` |
| `dispute_submitted` | `result_id`, `challenger`, `tx_hash` |
| `dispute_failed` | `result_id`, `challenger`, `error` |
| `prover_slashed` | `result_id` (emitted when prover loses dispute) |

---

## 2. Verification Process

An auditor can verify any inference result through three independent methods.

### 2.1 On-Chain Verification (Highest Assurance)

For a result that was challenged and resolved on-chain:

1. Query `TEEMLVerifier.getResult(resultId)` to retrieve the full result record.
2. Check `disputeResolved[resultId]` and `disputeProverWon[resultId]` to see the dispute outcome.
3. Query the `DisputeResolved` event for the transaction hash containing the ZK proof verification.
4. The `RemainderVerifier` emitted `ProofVerified` or `DAGProofVerified` confirming the proof checked out.

This is the strongest assurance: Ethereum consensus guarantees the verification was executed correctly.

### 2.2 Off-Chain Verification via REST API

The `services/verifier/` provides a stateless HTTP service for proof verification:

```
POST /circuits/:circuit_id/verify
{
  "proof_hex": "0x...",
  "public_inputs_hex": "0x..."
}

Response:
{
  "verified": true,
  "circuit_hash": "0x..."
}
```

Steps for an auditor:

1. Retrieve the `ProofBundle` JSON from the proof archive.
2. Register the circuit with the verifier service (`POST /circuits` with `gens_hex` and `dag_circuit_description`).
3. Submit the proof for verification (`POST /circuits/:id/verify`).
4. The service runs the full GKR+Hyrax verification locally and returns `verified: true/false`.

### 2.3 Off-Chain Verification via CLI / Library

The `zkml-verifier` crate provides direct programmatic verification:

```rust
use zkml_verifier::{ProofBundle, verify};

let bundle = ProofBundle::from_file("proof_bundle.json")?;
let result = verify(&bundle)?;
assert!(result.verified);
```

Or via the CLI binary:

```bash
zkml-verifier verify --bundle proof_bundle.json
```

The verifier is a standalone Rust crate with no blockchain dependencies. It implements the same GKR+Hyrax protocol as the Solidity contracts and the Stylus WASM verifier.

### 2.4 What Verification Confirms

A successful verification confirms:

- **Computation integrity:** The declared output was produced by executing the registered circuit (model) on the committed inputs.
- **Model binding:** The circuit hash in the proof matches a specific model topology (tree structure, layer count, connectivity).
- **Input commitment:** The input features are bound to the proof via Pedersen commitments. The proof cannot be reused for different inputs.

A successful verification does NOT confirm:

- **Input correctness:** The proof shows the computation was done correctly on *some* input, but does not prove the input represents real-world data.
- **Model quality:** The proof confirms the model ran as designed, not that the model is accurate or fair.
- **Timeliness:** The proof confirms a computation happened, not when it happened relative to external events.

---

## 3. Compliance Considerations

### 3.1 ECOA / FCRA (US Credit Decisions)

The Equal Credit Opportunity Act and Fair Credit Reporting Act require that adverse credit decisions be explainable and auditable.

**How World ZK Compute supports compliance:**

- **Model provenance:** The `modelHash` provides a cryptographic binding to a specific model version. Regulators can verify that the model used for a decision matches the model that was reviewed and approved.
- **Decision reproducibility:** The ZK proof cryptographically guarantees that the output was produced by the declared model on the declared inputs. Given the model weights and input features, anyone can reproduce and verify the decision.
- **Audit trail immutability:** On-chain records (events + state) are immutable once finalized. Off-chain proof archives use append-only storage.
- **Input privacy:** The `inputHash` allows auditors to verify that specific inputs were used without storing raw PII on-chain. Raw features are stored only in the off-chain proof archive (access-controlled).

**Gaps:**
- The system does not provide model explainability (e.g., SHAP values, feature importance). This must be handled by the application layer.
- Adverse action notices require human-readable explanations, not cryptographic proofs. The proof provides the underlying audit trail, not the consumer-facing explanation.

### 3.2 EU AI Act (High-Risk AI Systems)

The EU AI Act (effective August 2026) imposes requirements on high-risk AI systems including credit scoring, employment, and essential services.

**How World ZK Compute supports compliance:**

- **Technical documentation (Art. 11):** The circuit description, model hash, and proof bundle provide a complete technical record of how each decision was computed.
- **Record-keeping (Art. 12):** The proof archive maintains timestamped, tamper-evident records of every inference. On-chain events provide an independent, immutable log.
- **Transparency (Art. 13):** The circuit structure (registered on-chain) documents the model topology. The `modelHash` provides version traceability.
- **Human oversight (Art. 14):** The challenge mechanism allows any party to dispute a result within the challenge window, providing a human-in-the-loop override path.
- **Accuracy and robustness (Art. 15):** The ZK proof mathematically guarantees computation accuracy. The proof is deterministic and reproducible.

**Gaps:**
- The AI Act requires risk assessments and conformity assessments before deployment. These are organizational requirements, not technical ones.
- Data governance requirements (Art. 10) must be handled by the data pipeline, not the proof system.

### 3.3 GDPR (Data Protection)

**How World ZK Compute supports compliance:**

- **Data minimization:** Only `inputHash` (a keccak256 digest) is stored on-chain. Raw input features are not exposed in the proof or on-chain record.
- **Pedersen commitment hiding:** Input features in the ZK proof are protected by Pedersen commitments. Under the discrete log assumption, commitments are computationally hiding -- the proof reveals nothing about input values.
- **Right to erasure:** On-chain records cannot be deleted (blockchain immutability). However, on-chain data contains only hashes, not personal data. Off-chain proof archives containing raw features can be purged in compliance with erasure requests (the on-chain audit trail remains intact via hashes).
- **Pseudonymization:** The `submitter` and `enclave` addresses are pseudonymous. Mapping addresses to natural persons requires off-chain records.

**Gaps:**
- If inference outputs (stored in the `result` field on-chain) contain personal data or can be used to re-identify individuals, GDPR obligations apply to that data. Applications should hash or encrypt outputs before on-chain storage if they may contain personal data.
- Cross-border data transfer considerations apply if the TEE enclave, operator, and blockchain nodes are in different jurisdictions.

---

## 4. Storage Recommendations

### 4.1 Proof Archive Structure

The operator's `ProofArchive` uses a date-partitioned, append-only directory structure:

```
$PROOFS_DIR/
  2026-03-26/
    proof-a1b2c3d4-...json
    proof-e5f6g7h8-...json
  2026-03-27/
    proof-i9j0k1l2-...json
```

Files are written atomically (write to temp file, then rename) to prevent partial writes.

### 4.2 Immutable Storage

For production deployments, proof archives should be stored on immutable/WORM (Write Once, Read Many) storage:

| Storage Backend | Mechanism | Suitability |
|---|---|---|
| **AWS S3 Object Lock** (Compliance mode) | Objects cannot be deleted or overwritten for the retention period, even by root | Recommended for AWS deployments |
| **AWS S3 Glacier** with Vault Lock | Vault lock policy enforces WORM; cannot be changed once locked | Long-term archival |
| **Azure Blob Storage** (immutable policies) | Time-based or legal hold retention | Azure deployments |
| **GCP Cloud Storage** (retention policies) | Bucket-level or object-level retention locks | GCP deployments |
| **On-chain (IPFS + Ethereum)** | Store proof bundle CID on-chain; IPFS pins the data | Maximum decentralization; highest cost |

### 4.3 Retention Periods

| Use Case | Recommended Retention | Basis |
|---|---|---|
| **Financial decisions (US)** | 7 years minimum | ECOA adverse action records (Regulation B, 12 CFR 1002.12) |
| **Credit reporting (US)** | 7 years minimum | FCRA record retention |
| **High-risk AI (EU)** | Duration of AI system lifecycle + 10 years | EU AI Act Art. 12(4) (logs for high-risk systems) |
| **General compliance** | 5 years minimum | Common financial services retention floor |
| **Internal audit** | 3 years minimum | Organizational policy; sufficient for most internal reviews |

### 4.4 Integrity Verification

To verify archive integrity over time:

1. **Hash chain:** Maintain a running hash chain of all archived proofs: `H_n = SHA-256(H_{n-1} || proof_n)`. Store periodic checkpoints on-chain or in a separate trusted store.
2. **Cross-reference on-chain:** For any archived proof, verify `keccak256(result)` matches the on-chain `resultHash` for the corresponding `resultId`.
3. **Periodic re-verification:** Run `zkml-verifier verify` on sampled proof bundles to confirm they remain valid (guards against archive corruption).

### 4.5 Access Control

| Data | Sensitivity | Access |
|---|---|---|
| On-chain events and state | Public | Anyone (blockchain is public) |
| Proof bundles (without features) | Low | Auditors, compliance, engineering |
| Proof bundles (with features) | High (may contain PII) | Compliance and legal only; access-logged |
| Operator audit logs | Medium | Security, compliance, SRE |
| Enclave signing keys | Critical | TEE enclave only (never exported) |
| Admin contract keys | Critical | Multisig signers only |

---

## 5. Audit Checklist

For auditors reviewing a specific inference result:

- [ ] Retrieve `resultId` from on-chain `ResultSubmitted` event
- [ ] Query `TEEMLVerifier.getResult(resultId)` for full record
- [ ] Verify `modelHash` matches the approved model version
- [ ] Verify `enclave` address was registered and active at `submittedAt` time
- [ ] Check `enclaveImageHash` for the enclave matches the approved enclave image
- [ ] If challenged: verify `DisputeResolved` event and `disputeProverWon` outcome
- [ ] If proof exists: download `ProofBundle` from archive
- [ ] Run `zkml-verifier verify --bundle <file>` to independently confirm proof validity
- [ ] Verify `result` bytes decode to expected output format (e.g., JSON prediction scores)
- [ ] Cross-reference `inputHash` against off-chain input records (if available)
- [ ] Verify archive entry `archived_at` timestamp is consistent with on-chain `submittedAt`
