# World ZK Compute -- Live Demo Script

**Audience:** Bank stakeholders, compliance officers, regulators
**Duration:** 10 minutes + Q&A
**Prerequisites:** Docker, curl

---

## Setup (before the call)

Start the off-chain proof pipeline. This brings up three services behind a
single API gateway (proof generator, proof registry, verifier) and a Prometheus
container, whose config (`deploy/prometheus.yml`) has no scrape target for these
services. The compose file starts the proof generator with `--mock`: it does not
evaluate the model or run a prover, so Step 3 returns a placeholder instead of a
proof and Steps 4-5 cannot complete with it (see the notes at those steps).

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

> **Tip:** The on-chain variant at the end of this document mentions
> `docker compose -f docker-compose.bank-demo.yml --profile onchain up -d`.
> That file starts only the verifier API and a local Anvil node (no gateway,
> registry or generator), so it does not replace the stack above for Steps 2-6.

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

Submit a credit application's feature vector. Outside mock mode the generator
forwards the request to an `xgboost-remainder` prover for the inference and a
GKR+Hyrax proof of it; the demo image does not ship that prover. This demo stack
runs in mock mode, where neither happens: `output` is "approved" whenever the
features sum to more than zero, and `proof_hex` is a 32-byte SHA-256
placeholder, not a proof.

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

> "The output says this applicant is approved. But the important part is
> the proof -- it is a blob that anyone can verify without access to the
> model weights or the customer data."

The mock response carries no score, `prove_time_ms` is 0 and the "proof" is 32
bytes, so the demo gives no proving time or proof size to quote.

### Step 4: Verify the Proof (1 min)

Anyone with the proof can verify it independently. The verifier service
implements the same GKR+Hyrax protocol as the on-chain Solidity contracts.

**Cannot complete with the proof from Step 3 today.** The registry never
receives it: the compose file does not set `REGISTRY_URL` on the generator, and
with it set the registry rejects the mock bundle (HTTP 422, missing field
`gens_hex`). With the `proof_id` from Step 3 the call below returns
`404 proof '...' not found`. It does work for a real proof bundle stored by hand:

```bash
# Store the repository's fixture bundle in the registry; use the returned "id" as PROOF_ID
curl -s -X POST http://localhost:8080/proofs \
  -H "Content-Type: application/json" \
  -d @contracts/test/fixtures/phase1a_dag_fixture.json | python3 -m json.tool
```

```bash
# Retrieve the proof bundle from the registry and verify it
# (PROOF_ID is the "id" returned by POST /proofs, not the proof_id from Step 3)
PROOF_ID="829d982a-69af-4555-9036-2489ea5de290"

curl -s -X POST "http://localhost:8080/proofs/${PROOF_ID}/verify" \
  | python3 -m json.tool
```

Expected output (for the fixture bundle):

```json
{
  "verified": true,
  "receipt_id": "4b4d380d-c14e-400a-88f1-2114b24cc6a6"
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

Like Step 4, this returns 404 (`no verification receipt for proof '...'`) for
the `proof_id` from Step 3. Expected output for the fixture bundle verified in
Step 4 (`model_hash` is empty because the fixture bundle carries none; `verifier_host` is `VERIFIER_HOST` if set, else the registry's `HOSTNAME`, else `unknown`):

```json
{
  "receipt_id": "4b4d380d-c14e-400a-88f1-2114b24cc6a6",
  "proof_id": "829d982a-69af-4555-9036-2489ea5de290",
  "circuit_hash": "0x8e7c...e909",
  "model_hash": "",
  "verified": true,
  "verified_at": "2026-11-24T06:49:56.360932+00:00",
  "verifier_version": "0.1.0",
  "verifier_host": "unknown",
  "signature": "6c5fff85...0b669ea8"
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

Expected output (transparency root, after the one fixture bundle from Step 4;
on a stack where nothing was stored the proof list is empty, `tree_size` is 0
and the root is all zeros):

```json
{
  "root": "25f4e401...e7ce0ad4",
  "tree_size": 1
}
```

> "Every proof is recorded in an append-only log. The Merkle root changes if
> any entry is modified or deleted. Your compliance team can verify any decision
> at any time. An auditor can independently check that the log has not been
> tampered with by verifying the Merkle inclusion proof for any specific entry."

```bash
# Verify a specific entry is in the transparency log
# (404 "leaf 0 not found" while the log is empty)
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

> "Two model types supported today: XGBoost and LightGBM. Python SDK, REST
> API, Rust library, or CLI -- deploy in your VPC, no blockchain required for
> off-chain verification."

```bash
# Nothing is published to a registry yet (PyPI and npm return 404, and the
# crates.io name zkml-verifier belongs to an unrelated project), so install
# from a checkout of this repository:
pip install ./crates/zkml-verifier/python   # Python verifier
cargo install --path crates/zkml-verifier   # Rust CLI
# JavaScript/WASM: build with wasm-pack, see docs/PUBLISHING.md
```

---

## Q&A Talking Points

| Question | Answer |
|---|---|
| "What models do you support?" | XGBoost and LightGBM. The random forest converter is broken and logistic regression is rejected by the prover CLI (see the Supported Models table in the README). Neural network support is in development. |
| "How long does proving take?" | 4-5 seconds for an 88-layer XGBoost model. (The demo stack cannot show this: it runs in mock mode.) |
| "What about larger models?" | Tree-based models cover most banking ML (credit scoring, fraud, AML). Neural network support is on the roadmap. |
| "Is this on mainnet?" | No, and not on Arbitrum Sepolia either: `deployments/arbitrum-sepolia.json` records no deployed contract. The Solidity ZK verifier contract (`RemainderVerifier`, 136,142 bytes) is over the 24,576-byte contract size limit of Ethereum, Arbitrum One and World Chain and of their Sepolia testnets, so of the Solidity contracts only the TEE contract can be deployed on them. |
| "What is the gas cost?" | 3-6M gas on Arbitrum (~$0.02) for ZK dispute resolution via hybrid Stylus+Groth16. TEE happy path is ~$0.0001. |
| "Can we use our existing models?" | Yes, for XGBoost and LightGBM models exported as JSON. No retraining needed. |
| "How does this help with EU AI Act?" | Art. 11 (technical docs), Art. 12 (record-keeping), Art. 13 (transparency), Art. 15 (accuracy) -- see docs/AUDIT_TRAIL.md Section 3.2. |
| "What about GDPR?" | Only hashes are stored on-chain, never PII. Input features are protected by Pedersen commitments in proofs. Raw features stored only in access-controlled off-chain archives. |
| "Can this be self-hosted?" | Yes. Docker Compose for development, Helm/Kustomize for Kubernetes, or bare-metal. No external dependencies required. |
| "What is the uptime guarantee?" | Dispute windows (1h challenge + 24h resolution) provide a safety margin. docs/DISASTER_RECOVERY.md sets recovery-time targets of 5 to 30 minutes per service. |

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
# (See scripts/tee-dispute-e2e.sh for the full automated flow;
#  scripts/e2e-test.sh is the risc0 ExecutionEngine flow)
```

**These commands cannot complete today.** The forge command does not compile
from the repository root, the script reads `DEPLOYER_KEY` and requires
`REMAINDER_VERIFIER`, and the compose Anvil keeps the 24,576-byte contract size
limit, so the 136,142-byte RemainderVerifier cannot be deployed on it. What does
run is the script, which starts its own Anvil (port 8552, 500M gas limit, code
size limit disabled), deploys both verifiers, submits a result, challenges it
and resolves the dispute with the fixture proof:

```bash
bash scripts/tee-dispute-e2e.sh
```

This demonstrates the full TEE attestation -> challenge -> ZK dispute -> settlement
cycle on a local chain with 500M gas limit (sufficient for direct GKR verification).
