# GPU Deployment Guide

Deploy the World ZK Compute prover on multi-GPU cloud instances.

## Cloud Instance Types

| Provider | Instance | GPUs | VRAM | vCPUs | Use Case |
|----------|----------|------|------|-------|----------|
| AWS | `g5.xlarge` | 1x A10G | 24 GB | 4 | Dev/small |
| AWS | `g5.12xlarge` | 4x A10G | 96 GB | 48 | Production |
| GCP | `a2-highgpu-1g` | 1x A100 | 40 GB | 12 | Dev/medium |
| GCP | `a2-highgpu-4g` | 4x A100 | 160 GB | 48 | Production |
| Azure | `NC24ads_A100_v4` | 1x A100 | 80 GB | 24 | Medium |

## Quick Start (Docker)

```bash
# 1. Set environment
export PRIVATE_KEY="0x..."
export RPC_URL="https://worldchain-mainnet.g.alchemy.com/v2/..."
export ENGINE_ADDRESS="0x..."

# 2. Build and run
docker compose -f docker-compose.gpu.yml up -d

# 3. Check health
curl http://localhost:8081/health
curl http://localhost:8081/status
```

## Native Build (Linux + CUDA)

```bash
# Install CUDA toolkit 12.x
# Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build with CUDA
cd prover
cargo build --release --features cuda

# Run with auto-detected GPUs
./target/release/world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --proving-mode gpu-fallback \
  --use-snark

# Run with explicit GPU count (e.g., 4x A100)
./target/release/world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --proving-mode gpu-fallback \
  --max-gpu-concurrent 4 \
  --max-cpu-concurrent 8 \
  --use-snark
```

## Local Dev (macOS + Metal)

```bash
cd prover
cargo build --release --features metal

./target/release/world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --proving-mode gpu-fallback
```

## Configuration

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--max-gpu-concurrent` | `MAX_GPU_CONCURRENT` | 0 (auto) | GPU proving slots |
| `--max-cpu-concurrent` | `MAX_CPU_CONCURRENT` | 0 (auto) | CPU proving slots |
| `--max-concurrent` | `MAX_CONCURRENT` | 4 | Overall job limit |
| `--proving-mode` | `PROVING_MODE` | gpu-fallback | Proving strategy |
| `--use-snark` | `USE_SNARK` | false | Enable Groth16 |

## Monitoring

```bash
# Prometheus metrics
curl http://localhost:8081/metrics

# Key GPU metrics:
#   prover_gpu_device_count    - Number of GPUs
#   prover_gpu_jobs_total      - Completed GPU jobs
#   prover_gpu_jobs_in_progress - Active GPU jobs

# Detailed status (includes GPU device info)
curl http://localhost:8081/status | jq .
```
