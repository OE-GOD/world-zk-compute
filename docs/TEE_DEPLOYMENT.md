# TEE Deployment Guide

This guide covers building, deploying, and operating the World ZK Compute TEE enclave on AWS Nitro Enclaves.

## Architecture Overview

The TEE enclave runs ML inference (XGBoost/LightGBM) inside an isolated AWS Nitro Enclave. It produces ECDSA-signed attestations that can be verified on-chain by the `TEEMLVerifier` contract.

```
                  vsock proxy
  Client --> EC2 Parent Host ----------> Nitro Enclave
               (port 5000)                tee-enclave
                                          - ML inference
                                          - ECDSA signing
                                          - Nitro attestation
```

**Key properties:**
- The enclave has no persistent storage, no network access, and no interactive shell
- Communication happens exclusively via vsock (virtual socket)
- The enclave's code is measured at boot; the PCR0 hash uniquely identifies the image
- The PCR0 is registered on-chain so verifiers can confirm attestation provenance

## Prerequisites

### EC2 Instance Requirements

- **Instance type**: Any Nitro-enabled instance (e.g., `m5.xlarge`, `c5.2xlarge`, `r5.large`)
  - Must support Nitro Enclaves (check [AWS docs](https://docs.aws.amazon.com/enclaves/latest/user/nitro-enclave.html) for supported types)
- **AMI**: Amazon Linux 2 or Amazon Linux 2023 recommended
- **Enclave support**: Must be enabled at instance launch (`--enclave-options Enabled=true`)
- **Resources**: At least 2 vCPUs and 512 MB memory dedicated to the enclave

### Software Dependencies

```bash
# Install Docker
sudo yum install -y docker
sudo systemctl enable docker && sudo systemctl start docker

# Install Nitro CLI
sudo amazon-linux-extras enable aws-nitro-enclaves-cli  # AL2
sudo yum install -y aws-nitro-enclaves-cli aws-nitro-enclaves-cli-devel

# Install jq (for measurement extraction)
sudo yum install -y jq

# Configure enclave allocator (set CPU and memory limits)
sudo tee /etc/nitro_enclaves/allocator.yaml <<EOF
---
memory_mib: 512
cpu_count: 2
EOF

# Start the allocator service
sudo systemctl enable nitro-enclaves-allocator && sudo systemctl start nitro-enclaves-allocator

# Add your user to the docker and ne groups
sudo usermod -aG docker $USER
sudo usermod -aG ne $USER
# Log out and back in for group changes to take effect
```

## Building the Enclave Image

### Option A: From the enclave crate (minimal)

Builds only the `tee-enclave` crate. Model must be supplied at runtime.

```bash
cd tee/enclave
./build-eif.sh --output-dir /tmp/eif
```

### Option B: From the top-level tee directory (includes test model)

Builds the enclave with the test model baked in (useful for testing).

```bash
cd tee/scripts
./build-enclave.sh --nitro --output-dir /tmp/eif
```

Both produce:
- `enclave.eif` (or `tee-enclave.eif`) -- the Enclave Image File
- `enclave-measurements.json` -- PCR0, PCR1, PCR2 values

### Build Output

```
=== Build Complete ===
EIF:  /tmp/eif/enclave.eif
PCR0: 000102030405...  (48 bytes, hex-encoded)
PCR1: ...
PCR2: ...
```

**Important:** Save the PCR0 value. It changes whenever the Docker image changes (code, dependencies, base image, or model if baked in).

## Running the Enclave

### Step 1: Start the Enclave

```bash
# Using the deploy script
CPU_COUNT=2 MEMORY_MB=512 tee/scripts/deploy-enclave.sh /tmp/eif/enclave.eif

# Or directly with nitro-cli
nitro-cli run-enclave \
    --eif-path /tmp/eif/enclave.eif \
    --cpu-count 2 \
    --memory 512
```

The command returns the Enclave ID:

```json
{
  "EnclaveID": "i-0abc123def456-enc0123456789abcdef0",
  "EnclaveCID": 16,
  "NumberOfCPUs": 2,
  "MemoryMiB": 512
}
```

### Step 2: Set Up the vsock Proxy

The enclave listens on port 5000 inside the vsock. You need a proxy on the parent instance to forward TCP traffic:

```bash
# Install vsock-proxy if not present
sudo yum install -y aws-nitro-enclaves-cli

# Forward TCP port 5000 on localhost to vsock CID 16, port 5000
vsock-proxy 5000 --local-port 5000 --remote-cid 16 --remote-port 5000 &
```

Alternatively, use `socat` or a custom proxy.

### Step 3: Verify the Enclave is Running

```bash
# Check enclave status
nitro-cli describe-enclaves

# Health check (via the vsock proxy)
curl -s http://localhost:5000/health | jq .

# Get enclave info
curl -s http://localhost:5000/info | jq .

# Get attestation document
curl -s http://localhost:5000/attestation | jq .
```

### Step 4: Test Inference

```bash
curl -s -X POST http://localhost:5000/infer \
    -H "Content-Type: application/json" \
    -d '{"features": [5.0, 3.5, 1.5, 0.3]}' | jq .
```

Expected response:

```json
{
  "result": "0x...",
  "model_hash": "0x...",
  "input_hash": "0x...",
  "result_hash": "0x...",
  "attestation": "0x...",
  "enclave_address": "0x..."
}
```

## Registering PCR0 On-Chain

The on-chain `TEEMLVerifier` contract must know the expected PCR0 so it can verify that attestations came from a legitimate enclave build.

### Using the Registration Script

```bash
tee/scripts/register-enclave.sh \
    --use-attestation \
    --expected-pcr0 <PCR0_FROM_BUILD> \
    --enclave-url http://localhost:5000 \
    --verifier <TEEMLVerifier_CONTRACT_ADDRESS> \
    --rpc-url <RPC_URL> \
    --private-key <ADMIN_PRIVATE_KEY>
```

This script:
1. Fetches the `/attestation` endpoint from the running enclave
2. Validates that the returned PCR0 matches `--expected-pcr0`
3. Calls `TEEMLVerifier.registerEnclave(address, bytes32)` on-chain

### Manual Registration

```bash
# Get the enclave address and PCR0
ATTESTATION=$(curl -s http://localhost:5000/attestation)
ENCLAVE_ADDR=$(echo "$ATTESTATION" | jq -r '.enclave_address')
PCR0=$(echo "$ATTESTATION" | jq -r '.pcr0')

# Use first 32 bytes of PCR0 (48 bytes total) as image hash
IMAGE_HASH="0x${PCR0:0:64}"

# Register via cast
cast send \
    --rpc-url <RPC_URL> \
    --private-key <ADMIN_KEY> \
    <TEEMLVerifier_ADDRESS> \
    "registerEnclave(address,bytes32)" \
    "$ENCLAVE_ADDR" \
    "$IMAGE_HASH"
```

## Configuration

The enclave reads all configuration from environment variables. These are set in the Dockerfile or overridden at runtime.

| Variable | Default | Description |
|---|---|---|
| `MODEL_PATH` | `/app/model/model.json` | Path to the XGBoost/LightGBM model file |
| `MODEL_FORMAT` | `auto` | Model format: `auto`, `xgboost`, or `lightgbm` |
| `PORT` | `8080` (dev) / `5000` (Nitro) | HTTP listen port |
| `NITRO_ENABLED` | `false` (dev) / `true` (Nitro) | Enable real NSM attestation |
| `CHAIN_ID` | `1` | Chain ID for replay protection |
| `ENCLAVE_PRIVATE_KEY` | (random) | Hex-encoded secp256k1 key; omit for auto-generation |
| `ADMIN_API_KEY` | (none) | API key for admin endpoints; unset = disabled |
| `MAX_REQUESTS_PER_MINUTE` | `120` | Rate limit for `/infer` endpoint |
| `WATCHDOG_ENABLED` | `true` | Background health monitor |
| `EXPECTED_MODEL_HASH` | (none) | SHA-256 hex hash; rejects mismatched models |
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `warn`, `error`) |

## API Endpoints

| Method | Path | Description |
|---|---|---|
| GET | `/health` | Basic health check |
| GET | `/health/detailed` | Detailed health with watchdog status |
| GET | `/info` | Enclave info (address, model hash, features) |
| GET | `/attestation` | Nitro attestation document (CBOR COSE_Sign1) |
| POST | `/infer` | Run inference, return signed result |
| GET | `/metrics` | Request counters and latency stats |
| POST | `/admin/reload-model` | Hot-reload model (requires `ADMIN_API_KEY`) |
| GET | `/models` | List loaded models |
| POST | `/models/load` | Load a new model |
| DELETE | `/models/{model_id}` | Unload a model |

## Monitoring and Troubleshooting

### Viewing Enclave Logs

Nitro enclaves have no direct console access. Use the Nitro CLI debug mode:

```bash
# Run enclave in debug mode (allows console output)
nitro-cli run-enclave \
    --eif-path enclave.eif \
    --cpu-count 2 \
    --memory 512 \
    --debug-mode

# Attach to console
nitro-cli console --enclave-id <ENCLAVE_ID>
```

**Warning:** Debug mode disables attestation security guarantees. Never use in production.

### Common Issues

**Enclave fails to start:**
- Check allocator config: `cat /etc/nitro_enclaves/allocator.yaml`
- Ensure enough CPUs/memory are allocated: `nitro-cli describe-enclaves`
- Verify the allocator service is running: `systemctl status nitro-enclaves-allocator`

**Health check fails:**
- Verify the vsock proxy is running and forwarding to the correct CID/port
- Check that the enclave has enough memory (512 MB minimum recommended)
- Check the model file is accessible inside the enclave

**Attestation returns `is_nitro: false`:**
- Binary may not be compiled with `--features nitro`
- May be running in debug mode or outside a real Nitro enclave
- Check `NITRO_ENABLED=true` in the environment

**PCR0 mismatch after rebuild:**
- PCR0 changes whenever ANY part of the Docker image changes
- Ensure you rebuild and re-register on-chain after any code or dependency change
- Pin Rust toolchain and base image versions for reproducible builds

**Rate limiting (HTTP 429):**
- Default: 120 requests/minute
- Adjust with `MAX_REQUESTS_PER_MINUTE` environment variable

### Health Monitoring

The enclave includes a built-in watchdog that monitors:
- Inference latency trends
- Error rates
- Memory pressure (via request timing)

Access detailed health via `GET /health/detailed`:

```json
{
  "status": "healthy",
  "uptime_secs": 3600,
  "total_requests": 1234,
  "error_count": 2,
  "avg_latency_ms": 5.2,
  "watchdog": { "status": "ok", "last_check": "2025-01-01T12:00:00Z" }
}
```

## Security Considerations

1. **Key management**: In production, omit `ENCLAVE_PRIVATE_KEY` to let the enclave generate a random key. The key is bound to the enclave image via Nitro attestation and exists only in enclave memory.

2. **Model integrity**: Set `EXPECTED_MODEL_HASH` to the SHA-256 hash of your model file. The enclave will refuse to start if the model does not match.

3. **Replay protection**: The `/infer` endpoint supports `nonce` and `timestamp` fields. Clients should always provide these to prevent replay attacks.

4. **Admin endpoints**: Set `ADMIN_API_KEY` to a strong secret. Without it, admin endpoints (model reload) are disabled entirely.

5. **Network isolation**: Nitro enclaves have no network access by default. All communication goes through vsock, which the parent instance controls.

## Updating the Enclave

1. Make code or model changes
2. Rebuild the EIF: `./build-eif.sh`
3. Note the new PCR0 (it will change)
4. Terminate the old enclave: `nitro-cli terminate-enclave --enclave-id <OLD_ID>`
5. Start the new enclave: `nitro-cli run-enclave ...`
6. Register the new PCR0 on-chain
7. Optionally deregister the old enclave address on-chain

## Local Development

For local testing without Nitro hardware:

```bash
# Using docker-compose (dev mode, no Nitro attestation)
cd tee/
docker compose up

# Or run the test script
tee/scripts/test-local.sh
```

In dev mode (`NITRO_ENABLED=false`), the enclave produces mock attestation documents that are structurally identical but not cryptographically valid.
