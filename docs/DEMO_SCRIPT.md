# Bank Demo Walkthrough

Step-by-step script for presenting the verifiable AI inference demo to bank stakeholders.

## Setup (5 minutes before)

```bash
# Start the off-chain stack
docker compose -f docker-compose.offchain.yml up -d

# Verify services are running
worldzk health
```

Expected output:
```
Status: ok
  verifier: "healthy"
  registry: "healthy"
  generator: "healthy"
```

## Demo Flow (15 minutes)

### 1. The Problem (2 min)

"Banks use ML models for credit scoring, fraud detection, and AML. Regulators and auditors need proof that the model ran correctly — but they can't see the model weights (proprietary) or customer data (PII).

World ZK Compute solves this: we generate a cryptographic proof that the model ran correctly, without revealing the model or the data."

### 2. Register a Model (1 min)

```bash
# Register the credit scoring model
curl -s -X POST http://localhost:8080/models \
  -H "Content-Type: application/json" \
  -d '{"id":"credit-v1","name":"Credit Scoring XGBoost","format":"xgboost","num_features":6}'
```

"We register the model with the system. In production, this would include the model hash for integrity verification."

### 3. Run Inference + Generate Proof (2 min)

```bash
# Good applicant
worldzk prove credit-v1 720,85000,12,0.22,5,1
```

"The system runs inference inside a TEE and generates a zero-knowledge proof. The proof takes ~4 seconds for an 88-layer XGBoost model."

### 4. Submit Proof to Registry (1 min)

```bash
# Submit the proof bundle
worldzk submit proof_output.json
```

"Every proof is stored in an append-only registry with a content hash for tamper evidence. This is your audit trail."

### 5. Verify the Proof (2 min)

```bash
# Verify independently
worldzk verify proof_output.json
```

"Anyone with the proof can verify it — no access to the model or data needed. This is what an auditor or regulator would do."

### 6. Search the Audit Trail (1 min)

```bash
worldzk search --limit 5
```

"The registry maintains a searchable history of all verified inferences. Filter by model, date, or verification status."

### 7. Cost Comparison (2 min)

| Approach | Cost | Trust | Privacy |
|----------|------|-------|---------|
| Re-run model | $0.10+ | Need model access | Exposes model |
| TEE attestation | $0.0001 | Hardware trust | Model stays private |
| ZK proof (dispute) | $0.02 | Math only | Model + data private |

"The happy path costs $0.0001 per inference. ZK proofs are only generated if someone challenges a result — like insurance for your AI."

### 8. Architecture (2 min)

```
Client → TEE Enclave → On-chain Submission → Challenge Window → Finalized
              ↓                                     ↓
         Attestation                          ZK Proof (if challenged)
```

"Three layers of defense: TEE hardware isolation, economic bonding (stake), and mathematical proof. No single point of failure."

## Q&A Talking Points

- **"What models do you support?"** — XGBoost, LightGBM, Random Forest, Logistic Regression. Any tree ensemble or linear model.
- **"How long does proving take?"** — 4-5 seconds for an 88-layer XGBoost model. Sub-second for logistic regression.
- **"What about larger models?"** — We're working on neural network support. Current focus is tree-based models which cover most banking ML.
- **"Is this on mainnet?"** — Arbitrum Sepolia testnet today. Mainnet deployment is ready, pending security audit.
- **"What's the gas cost?"** — 3-6M gas on Arbitrum (~$0.02) for ZK dispute resolution. TEE happy path is ~$0.0001.
- **"Can we use our existing models?"** — Yes. Export from scikit-learn, XGBoost, or LightGBM as JSON. No retraining needed.

## Cleanup

```bash
docker compose -f docker-compose.offchain.yml down
```
