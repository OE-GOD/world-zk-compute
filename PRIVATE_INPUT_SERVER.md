# Private Input Server - Implementation

Like Bonsol's approach: provers must prove they claimed the job on-chain before receiving private input data.

---

## What I Built

### 1. Private Input Client (`prover/src/private_input.rs`)

Client for provers to fetch private inputs with authentication.

```rust
let client = PrivateInputClient::new(config, wallet);
let input = client.fetch_input(request_id, input_id).await?;
```

**Features:**
- Signs requests with prover's wallet
- Server verifies signature + on-chain claim
- AES-256-GCM decryption
- Input digest verification

### 2. Private Input Server (`private-input-server/`)

Server that stores encrypted inputs and releases them only to authorized provers.

```bash
private-input-server \
  --rpc-url https://sepolia.infura.io/v3/... \
  --engine-address 0x9CFd1CF0e263420e010013373Ec4008d341a483e \
  --master-key <32-bytes-hex>
```

**API Endpoints:**
- `POST /inputs` - Upload input (returns inputId)
- `GET /inputs/:id` - Fetch input (requires auth headers)
- `DELETE /inputs/:id` - Delete input
- `GET /health` - Health check

---

## How It Works (Like Bonsol)

```
USER UPLOADS INPUT
│
├─ 1. POST /inputs with raw data
│     └─ Server encrypts with random AES key
│     └─ Server encrypts AES key with master key
│     └─ Returns: inputId (hash of encrypted data)
│
├─ 2. User submits job on-chain
│     └─ requestExecution(imageId, inputDigest, "private://<inputId>", ...)
│
PROVER CLAIMS & FETCHES
│
├─ 3. Prover claims job on-chain
│     └─ claimExecution(requestId)
│     └─ On-chain: request.claimer = proverAddress
│
├─ 4. Prover requests input from server
│     └─ Signs message: sign(requestId + timestamp)
│     └─ Headers: X-Request-Id, X-Prover-Address, X-Signature, X-Timestamp
│
├─ 5. Server verifies:
│     └─ Recover address from signature ✓
│     └─ Check on-chain: is this the claimer? ✓
│     └─ Check timestamp: within 5 minutes? ✓
│
├─ 6. Server returns encrypted data + decryption key
│
├─ 7. Prover decrypts locally and runs in zkVM
│
└─ 8. Prover submits proof on-chain
```

---

## Security Model

### Encryption

```
Upload:
  plaintext → AES-256-GCM encrypt → encrypted_data
                    ↑
              random_key → encrypt with master_key → stored_key

Fetch (authorized):
  stored_key → decrypt with master_key → random_key → sent to prover
  prover decrypts encrypted_data locally
```

### Authentication Flow

1. **Prover claims job** → recorded on-chain
2. **Prover signs request** → proves wallet ownership
3. **Server verifies signature** → recovers signer address
4. **Server checks on-chain** → is signer the claimer?
5. **Server returns data** → only if all checks pass

### Threat Mitigations

| Threat | Mitigation |
|--------|-----------|
| Unauthorized access | On-chain claim verification |
| Replay attacks | Timestamp + 5-minute window |
| Data breach | AES-256-GCM encryption at rest |
| Misbehaving prover | Blacklist + access logging |
| DoS | Rate limiting (TODO) |

---

## Comparison: IPFS vs Private Input Server

| Aspect | IPFS | Private Input Server |
|--------|------|---------------------|
| Access control | None - anyone with CID | On-chain claim verification |
| Encryption | None (or user-managed) | Server-managed AES-256-GCM |
| Who can fetch | Anyone | Only verified claimers |
| Revocation | Impossible | Instant deletion |
| Audit trail | None | Full access logging |
| Decentralization | High | Lower (single server) |

---

## Usage

### Start the Server

```bash
cd private-input-server
cargo run -- \
  --rpc-url $RPC_URL \
  --engine-address $ENGINE_ADDRESS \
  --master-key $(openssl rand -hex 32)
```

### Upload Input (User/Client)

```bash
# Encode data as base64
DATA=$(echo -n "my secret input" | base64)

# Upload
curl -X POST http://localhost:3000/inputs \
  -H "Content-Type: application/json" \
  -d "{\"data\": \"$DATA\"}"

# Response:
# {"input_id": "abc123...", "input_digest": "def456...", "expires_at": "..."}
```

### Submit Job On-Chain

```solidity
// Use private:// URL scheme
engine.requestExecution(
    imageId,
    inputDigest,
    "private://abc123...",  // The inputId from upload
    callbackContract,
    maxExpiry
);
```

### Prover Fetches (Automatic)

The prover client automatically:
1. Detects `private://` URL
2. Signs the request
3. Fetches and decrypts
4. Verifies input digest

```rust
// In prover code
let input = resolve_input(
    &request.input_url,
    request.id,
    Some(&private_client),
    Some(&ipfs_client),
).await?;
```

---

## Interview Talking Points

**Why Private Input Server over IPFS?**

> "I built a Private Input Server similar to Bonsol's approach. With IPFS, anyone with the content ID can fetch the data - there's no access control. With my Private Input Server, provers must authenticate by proving they've claimed the job on-chain. The server verifies the claim before releasing the encrypted data and decryption key.
>
> This gives us encryption at rest, access revocation, rate limiting, and a full audit trail - none of which are possible with IPFS."

**How does authentication work?**

> "The prover signs a message containing the request ID and timestamp. The server recovers the signer's address from the signature, then makes an RPC call to verify on-chain that this address is the current claimer of the job. Only if everything checks out does it release the data."

**What about the tradeoff with decentralization?**

> "That's the tradeoff. We lose IPFS's decentralization but gain strong access control. For sensitive data - especially at World's scale where you're handling identity proofs - I think access control is more important. We can add high availability with multiple servers behind a load balancer."
