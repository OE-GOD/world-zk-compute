# World ZK Compute -- Live Demo Script

**Audience:** Bank stakeholders, compliance officers, regulators
**Duration:** 10 minutes + Q&A
**Prerequisites:** Docker, curl

---

## Setup (before the call)

Start the off-chain proof pipeline. This brings up four services behind a
single API gateway: proof generator, proof registry, verifier, and Prometheus
metrics collection.

```bash
docker compose -f docker-compose.offchain.yml up -d
```

Verify all services are healthy:

```bash
curl -s http://localhost:8080/health | python3 -m json.tool
```

Expected output:

```json
{
  "status": "healthy",
  "services": [
    { "name": "generator", "healthy": true, "response_ms": 5 },
    { "name": "registry", "healthy": true, "response_ms": 2 },
    { "name": "verifier", "healthy": true, "response_ms": 3 }
  ],
  "healthy_count": 3,
  "total_count": 3
}
```

> **Tip:** If running the on-chain demo variant (Anvil local chain), use
> `docker compose -f docker-compose.bank-demo.yml --profile onchain up -d`
> instead. This adds a local Ethereum node for on-chain verification.

---

## Demo (10 minutes)

### Step 1: The Problem (1 min)

> "Your bank runs XGBoost models for credit decisions. A regulator asks: prove
> the right model ran on the right data, and the output was not tampered with.
>
> Today you say 'trust us.' With this system, you say 'here is the math.'"

Key talking point: Existing audit processes rely on re-running models (requires
exposing proprietary weights) or trusting infrastructure logs (forgeable). ZK
proofs provide a mathematical guarantee that no party can fake.

### Step 2: Upload a Model (1 min)

Register a credit scoring model with the proof generator. This records the
model hash (SHA-256 of the JSON weights) and auto-detects the model format.

```bash
# Run from the repository root; the request must carry the model file as "model_json"
curl -s -X POST http://localhost:8080/models \
  -H "Content-Type: application/json" \
  -d "{
    \"name\": \"credit-scoring-v1\",
    \"format\": \"xgboost\",
    \"model_json\": $(cat examples/bank-demo/models/credit_scoring.json)
  }" | python3 -m json.tool
```

Expected output:

```json
{
  "model_id": "cdabc04c-6495-4201-bc2f-c41120e38fbb",
  "model_hash": "0x200f...9a4a",
  "circuit_hash": "0xfb9d...4c9f",
  "format": "xgboost",
  "active": true
}
```

> "The model hash is a cryptographic fingerprint. It changes if even one weight
> in the model changes. This is how we bind proofs to specific model versions."

### Step 3: Run Inference + Generate Proof (2 min)

Submit a credit application's feature vector. The system runs the XGBoost model
and generates a GKR+Hyrax zero-knowledge proof that the inference was computed
correctly.

```bash
# MODEL_ID is the "model_id" returned by Step 2
MODEL_ID="cdabc04c-6495-4201-bc2f-c41120e38fbb"

curl -s -X POST http://localhost:8080/prove \
  -H "Content-Type: application/json" \
  -d "{
    \"model_id\": \"${MODEL_ID}\",
    \"features\": [52000, 0.38, 710, 4, 85000, 1]
  }" | python3 -m json.tool
```

Expected output (`docker-compose.offchain.yml` starts the generator with `--mock`):

```json
{
  "proof_id": "dd5084a8-ba4d-45eb-a2a3-42f40dc7d403",
  "model_hash": "0x200f...9a4a",
  "circuit_hash": "0xfb9d...4c9f",
  "output": "approved",
  "mock": true,
  "prove_time_ms": 0,
  "proof_bundle": {
    "proof_hex": "0xe2e3...c37c",
    "public_inputs_hex": "0xc3eb...d83c",
    "proof_size_bytes": 32
  }
}
```

> "The proof takes about 4 seconds for an 88-layer XGBoost model. The output
> score 0.87 means this applicant is likely approved. But the important part is
> the proof -- it is a 48KB blob that anyone can verify without access to the
> model weights or the customer data."

### Step 4: Verify the Proof (1 min)

Anyone with the proof can verify it independently. The verifier service
implements the same GKR+Hyrax protocol as the on-chain Solidity contracts.

```bash
# Retrieve the proof bundle from the registry and verify it
PROOF_ID="a1b2c3d4-e5f6-7890-abcd-ef1234567890"

curl -s -X POST "http://localhost:8080/proofs/${PROOF_ID}/verify" \
  | python3 -m json.tool
```

Expected output:

```json
{
  "verified": true,
  "receipt_id": "r-5678-abcd-ef01-234567890abc",
  "error": null
}
```

> "The verifier checked the zero-knowledge proof mathematically. No model
> access needed. No data access needed. The math is the audit."

### Step 5: Get Signed Receipt (1 min)

Every verification produces an ECDSA-signed receipt. This is a tamper-evident
record that a specific proof was verified at a specific time.

```bash
curl -s "http://localhost:8080/proofs/${PROOF_ID}/receipt" \
  | python3 -m json.tool
```

Expected output:

```json
{
  "receipt_id": "r-5678-abcd-ef01-234567890abc",
  "proof_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "circuit_hash": "0x8e7c...4f21",
  "model_hash": "0x7a3f...c821",
  "verified": true,
  "verified_at": "2026-03-27T10:01:15Z",
  "verifier_version": "0.1.0",
  "verifier_host": "verifier-api-1",
  "signature": "0x304402..."
}
```

> "This receipt is cryptographically signed. If anyone tampers with it, the
> signature breaks. Your compliance team can store these alongside loan
> decisions as evidence that the correct model processed the correct data."

### Step 6: Audit Trail (2 min)

The proof registry maintains a searchable, append-only history of all proofs.
Every submission is hashed into a Merkle-tree transparency log -- like
Certificate Transparency, but for AI decisions.

```bash
# List recent proofs
curl -s "http://localhost:8080/proofs?limit=5" | python3 -m json.tool

# Check the transparency log root
curl -s "http://localhost:8080/transparency/root" | python3 -m json.tool
```

Expected output (transparency root):

```json
{
  "root": "a4e3f7...b812c9",
  "tree_size": 42
}
```

> "Every proof is recorded in an append-only log. The Merkle root changes if
> any entry is modified or deleted. Your compliance team can verify any decision
> at any time. An auditor can independently check that the log has not been
> tampered with by verifying the Merkle inclusion proof for any specific entry."

```bash
# Verify a specific entry is in the transparency log
curl -s "http://localhost:8080/transparency/proof/0" | python3 -m json.tool
```

### Step 7: Cost & Architecture Summary (2 min)

| Verification Path | Cost | Trust | When Used |
|---|---|---|---|
| TEE attestation (happy path) | ~$0.0001 | TEE hardware | Normal operation |
| ZK proof off-chain | Free (CPU only) | Math only | Auditor spot-check |
| ZK proof on-chain (Arbitrum) | ~$0.02 (3-6M gas) | Math only | Dispute resolution |

> "The happy path costs a fraction of a cent. ZK proofs are generated only when
> someone challenges a result -- it is like insurance for your AI decisions.
> The on-chain path uses Arbitrum L2 for affordable dispute resolution."

Architecture overview:

```
Client --> TEE Enclave --> Attestation --> On-Chain Submission
                |                               |
                v                          1h window
          Signed Result                         |
                |                          Challenge?
                v                          /        \
         Proof Archive               No: Finalize   Yes: ZK Proof
              |                      (stake back)    (math settles)
              v
      Transparency Log
```

> "Three layers of defense. First, the TEE hardware isolates the model
> execution. Second, economic bonding means submitters put stake at risk.
> Third, zero-knowledge proofs provide a mathematical fallback that does not
> depend on trusting any hardware or any party."

---

## Closing (30 seconds)

> "Four model types supported today: XGBoost, LightGBM, Random Forest, and
> Logistic Regression. Python SDK, REST API, Rust library, or CLI -- deploy in
> your VPC, no blockchain required for off-chain verification."

```bash
pip install zkml-verifier    # Python SDK
npm install zkml-verifier    # JavaScript/WASM
cargo install zkml-verifier  # Rust CLI
```

---

## Q&A Talking Points

| Question | Answer |
|---|---|
| "What models do you support?" | XGBoost, LightGBM, Random Forest, Logistic Regression. Any tree ensemble or linear model. Neural network support is in development. |
| "How long does proving take?" | 4-5 seconds for an 88-layer XGBoost model. Sub-second for logistic regression. |
| "What about larger models?" | Tree-based models cover most banking ML (credit scoring, fraud, AML). Neural network support is on the roadmap. |
| "Is this on mainnet?" | Arbitrum Sepolia testnet today. Mainnet deployment is ready, pending third-party security audit. |
| "What is the gas cost?" | 3-6M gas on Arbitrum (~$0.02) for ZK dispute resolution via hybrid Stylus+Groth16. TEE happy path is ~$0.0001. |
| "Can we use our existing models?" | Yes. Export from scikit-learn, XGBoost, or LightGBM as JSON. No retraining needed. |
| "How does this help with EU AI Act?" | Art. 11 (technical docs), Art. 12 (record-keeping), Art. 13 (transparency), Art. 15 (accuracy) -- see docs/AUDIT_TRAIL.md Section 3.2. |
| "What about GDPR?" | Only hashes are stored on-chain, never PII. Input features are protected by Pedersen commitments in proofs. Raw features stored only in access-controlled off-chain archives. |
| "Can this be self-hosted?" | Yes. Docker Compose for development, Helm/Kustomize for Kubernetes, or bare-metal. No external dependencies required. |
| "What is the uptime guarantee?" | Dispute windows (1h challenge + 24h resolution) provide a safety margin. Full stack recovery time is under 15 minutes. See docs/DISASTER_RECOVERY.md. |

---

## Cleanup

```bash
docker compose -f docker-compose.offchain.yml down -v
```

---

## Alternate Demo: On-Chain Verification

For audiences interested in the on-chain dispute resolution path:

```bash
# Start local Ethereum node + verifier
docker compose -f docker-compose.bank-demo.yml --profile onchain up -d

# Deploy contracts to local Anvil
forge script contracts/script/DeployTEEMLVerifier.s.sol \
  --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast

# Submit a result, challenge it, resolve with ZK proof
# (See scripts/e2e-test.sh for the full automated flow)
```

This demonstrates the full TEE attestation -> challenge -> ZK dispute -> settlement
cycle on a local chain with 500M gas limit (sufficient for direct GKR verification).
