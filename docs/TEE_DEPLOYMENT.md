# TEE Deployment Guide

This guide covers building, deploying, and operating the World ZK Compute TEE enclave on AWS Nitro Enclaves.

## Architecture Overview

The TEE enclave runs ML inference (XGBoost/LightGBM) inside an isolated AWS Nitro Enclave. It produces ECDSA-signed attestations that can be verified on-chain by the `TEEMLVerifier` contract.

```
                  vsock bridge (not in this repository)
  Client --> EC2 Parent Host ----------> Nitro Enclave
               (port 5000)                tee-enclave (HTTP over TCP)
                                          - ML inference
                                          - ECDSA signing
                                          - Nitro attestation
```

**Key properties:**
- The enclave has no persistent storage, no network access, and no interactive shell
- A Nitro enclave's only channel to the outside is vsock (virtual socket). The `tee-enclave` binary does not speak vsock: it only opens a TCP listener (`0.0.0.0:$PORT`), and neither Nitro Dockerfile nor the build scripts add a vsock listener or a TCP-to-vsock bridge (see Step 2)
- The enclave's code is measured at boot; the PCR0 hash uniquely identifies the image
- The first 32 bytes of the PCR0 are recorded on-chain as the image hash of the enclave's signing address; the contract stores this value and does not check it (see "Registering PCR0 On-Chain")

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

Builds only the `tee-enclave` crate. The image contains no model file (`tee/enclave/Dockerfile.nitro` copies only the binary), and `tee-enclave` exits at startup when `MODEL_PATH` (`/app/model/model.json`) does not exist, so a model has to be added to the image before the EIF is built; `build-eif.sh` has no step for that.

```bash
cd tee/enclave
./build-eif.sh --output-dir /tmp/eif
```

### Option B: From the top-level tee directory (includes test model)

Builds the enclave with the test model baked in (useful for testing). `build-enclave.sh` builds `tee/Dockerfile` (the dev image: no `--features nitro`, `NITRO_ENABLED` not set, port 8080), not `tee/Dockerfile.nitro`, so the enclave in this EIF returns mock attestation documents (`is_nitro: false`).

```bash
cd tee/scripts
./build-enclave.sh --nitro --output-dir /tmp/eif
```

Both produce:
- `enclave.eif` (or `tee-enclave.eif`) -- the Enclave Image File
- `enclave-measurements.json` -- PCR0, PCR1, PCR2 values

### Build Output

From `build-eif.sh` (Option A); `build-enclave.sh --nitro` prints the same three PCR values after `Nitro EIF built:`.

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

`tee-enclave` serves HTTP on a TCP port inside the enclave: `0.0.0.0:5000` in the Option A image, `0.0.0.0:8080` in the Option B image. It does not listen on vsock, and neither image contains a TCP-to-vsock bridge. To reach it from the parent instance you need two pieces that this repository does not provide:

- inside the enclave image, a bridge from a vsock port to that TCP port;
- on the parent instance, a forwarder from a local TCP port to the enclave's vsock CID and port (`socat` or a custom proxy).

This guide gives no command for either piece, because none is included in the repository. The steps below assume a forwarder on the parent instance is listening on `localhost:5000`.

### Step 3: Verify the Enclave is Running

```bash
# Check enclave status
nitro-cli describe-enclaves

# Health check (via the forwarder from Step 2)
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
  "enclave_address": "0x...",
  "chain_id": 1,
  "nonce": 0,
  "timestamp": 1795502593
}
```

## Registering PCR0 On-Chain

`TEEMLVerifier.registerEnclave(address, bytes32)` (admin only) records the enclave's signing address together with an image hash, the first 32 bytes of PCR0. The contract only stores that hash: no check in the contract uses it (it is only returned by the `enclaves(address)` view), and the contract never sees an attestation document. What it checks, in `submitResult`, is that the result carries an EIP-712 signature (`TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)`, domain name `TEEMLVerifier`, version `1`, with the chain ID and the contract address) from an address that is registered and active. A signature made for another chain or another verifier address does not verify. Whoever registers the enclave is trusted to have checked the attestation document and the PCR0 off-chain.

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
2. Compares the `pcr0` field of that JSON response with `--expected-pcr0` (it does not verify the attestation document's signature or certificate chain)
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
| `PORT` | `8080`; `tee/enclave/Dockerfile.nitro` (Option A) sets `5000` | HTTP listen port (TCP, all interfaces) |
| `NITRO_ENABLED` | `false`; both `Dockerfile.nitro` files set `true` | Enable real NSM attestation |
| `CHAIN_ID` | `1` | Chain ID for replay protection |
| `TEE_VERIFIER_ADDRESS` | (none) | TEEMLVerifier address; when set (or sent as `verifier_address` in the request) `/infer` returns the EIP-712 signature that `submitResult` recovers |
| `ENCLAVE_PRIVATE_KEY` | (random) | Hex-encoded secp256k1 key; omit for auto-generation |
| `ADMIN_API_KEY` | (none) | API key for admin endpoints; unset = disabled |
| `MAX_REQUESTS_PER_MINUTE` | `120` | Rate limit for `/infer` endpoint |
| `WATCHDOG_ENABLED` | `true` | Background health monitor |
| `MAX_MODELS` | `5` | Capacity of the multi-model registry (`/models`) |
| `EXPECTED_MODEL_HASH` | (none) | SHA-256 hex hash; rejects mismatched models |
| `LOG_LEVEL` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`, `off`). The enclave does not read `RUST_LOG` |

## API Endpoints

| Method | Path | Description |
|---|---|---|
| GET | `/health` | Basic health check |
| GET | `/health/detailed` | Detailed health with watchdog status |
| GET | `/info` | Enclave info (address, model hash) |
| GET | `/attestation` | JSON with the attestation document (base64 CBOR COSE_Sign1 in `document`), `enclave_address`, `is_nitro` and `pcr0` |
| POST | `/infer` | Run inference, return signed result |
| GET | `/metrics` | Request counters and latency stats (JSON, not Prometheus format) |
| POST | `/admin/reload-model` | Hot-reload model (requires `ADMIN_API_KEY`) |
| GET | `/models` | List loaded models |
| POST | `/models/load` | Load a new model (requires `ADMIN_API_KEY`) |
| DELETE | `/models/{model_id}` | Unload a model (requires `ADMIN_API_KEY`) |

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
- Verify the vsock bridge and forwarder from Step 2 are running and forwarding to the correct CID/port
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
- Resident memory (read from `/proc/self/status` on Linux; `memory_rss_bytes` is 0 on other platforms)

Access detailed health via `GET /health/detailed`:

```json
{
  "status": "healthy",
  "error_rate": 0.0016,
  "avg_latency_ms": 5.2,
  "uptime_secs": 3600,
  "total_rate_limited": 0,
  "total_inferences": 1234,
  "total_errors": 2,
  "memory_rss_bytes": 52428800,
  "checks": { "error_rate": "ok", "latency": "ok" }
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

# Or, from the repository root, run the test script
tee/scripts/test-local.sh
```

In dev mode (`NITRO_ENABLED=false`), the enclave produces mock attestation documents that are structurally identical but not cryptographically valid.
