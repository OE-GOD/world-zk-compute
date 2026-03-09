#!/bin/bash
# Build the TEE enclave Docker image.
# For AWS Nitro: convert to EIF with nitro-cli after building.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "Building TEE enclave Docker image..."
docker build -t tee-enclave "$TEE_DIR"

echo ""
echo "Docker image built: tee-enclave"
echo ""
echo "To run locally:"
echo "  docker run -p 8080:8080 -e ENCLAVE_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 tee-enclave"
echo ""
echo "To convert to Nitro EIF:"
echo "  nitro-cli build-enclave --docker-uri tee-enclave --output-file tee-enclave.eif"
