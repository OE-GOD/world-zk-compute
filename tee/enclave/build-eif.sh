#!/usr/bin/env bash
set -euo pipefail

# Build an AWS Nitro Enclave Image File (EIF) from the tee-enclave crate.
#
# Must run on an EC2 instance with:
#   - Docker installed and running
#   - nitro-cli installed (amazon-linux-extras enable aws-nitro-enclaves-cli)
#   - Sufficient disk space for the Rust build (~2 GB)
#
# Usage:
#   ./build-eif.sh [--output-dir <DIR>] [--docker-tag <TAG>]
#
# Outputs:
#   <output-dir>/enclave.eif             — the enclave image file
#   <output-dir>/enclave-measurements.json — PCR0/PCR1/PCR2 measurements
#
# The PCR0 value must be registered on-chain for attestation verification:
#   register-enclave.sh --use-attestation --expected-pcr0 <PCR0>

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENCLAVE_DIR="$SCRIPT_DIR"
OUTPUT_DIR="$ENCLAVE_DIR"
DOCKER_TAG="worldzk-enclave"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --output-dir)  OUTPUT_DIR="$2"; shift 2 ;;
        --docker-tag)  DOCKER_TAG="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 [--output-dir <DIR>] [--docker-tag <TAG>]"
            exit 0
            ;;
        *) echo "Unknown argument: $1"; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Pre-flight checks
# ---------------------------------------------------------------------------
if ! command -v docker &>/dev/null; then
    echo "ERROR: docker is not installed."
    exit 1
fi

if ! command -v nitro-cli &>/dev/null; then
    echo "ERROR: nitro-cli is not installed."
    echo "Install it with: sudo amazon-linux-extras enable aws-nitro-enclaves-cli && sudo yum install -y aws-nitro-enclaves-cli"
    exit 1
fi

if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is not installed."
    echo "Install it with: sudo yum install -y jq"
    exit 1
fi

mkdir -p "$OUTPUT_DIR"

# ---------------------------------------------------------------------------
# Step 1: Build Docker image
# ---------------------------------------------------------------------------
echo "=== Building Nitro Enclave Docker Image ==="
echo "    Context:    $ENCLAVE_DIR"
echo "    Dockerfile: $ENCLAVE_DIR/Dockerfile.nitro"
echo "    Tag:        $DOCKER_TAG"
echo ""

docker build \
    -f "$ENCLAVE_DIR/Dockerfile.nitro" \
    -t "$DOCKER_TAG" \
    "$ENCLAVE_DIR"

echo ""
echo "Docker image built: $DOCKER_TAG"

# ---------------------------------------------------------------------------
# Step 2: Convert to EIF
# ---------------------------------------------------------------------------
EIF_PATH="$OUTPUT_DIR/enclave.eif"
MEASUREMENTS_PATH="$OUTPUT_DIR/enclave-measurements.json"

echo ""
echo "=== Converting to Nitro EIF ==="
echo "    Output: $EIF_PATH"
echo ""

BUILD_OUTPUT=$(nitro-cli build-enclave \
    --docker-uri "$DOCKER_TAG" \
    --output-file "$EIF_PATH" 2>&1)

echo "$BUILD_OUTPUT"

# ---------------------------------------------------------------------------
# Step 3: Extract and save measurements
# ---------------------------------------------------------------------------
PCR0=$(echo "$BUILD_OUTPUT" | jq -r '.Measurements.PCR0')
PCR1=$(echo "$BUILD_OUTPUT" | jq -r '.Measurements.PCR1')
PCR2=$(echo "$BUILD_OUTPUT" | jq -r '.Measurements.PCR2')

cat > "$MEASUREMENTS_PATH" <<MEOF
{
  "pcr0": "${PCR0}",
  "pcr1": "${PCR1}",
  "pcr2": "${PCR2}",
  "eif_path": "${EIF_PATH}",
  "docker_tag": "${DOCKER_TAG}",
  "built_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
MEOF

echo ""
echo "=== Build Complete ==="
echo "EIF:  $EIF_PATH"
echo "PCR0: $PCR0"
echo "PCR1: $PCR1"
echo "PCR2: $PCR2"
echo ""
echo "Measurements saved to: $MEASUREMENTS_PATH"
echo ""
echo "Save this PCR0 -- it must be registered on-chain for attestation verification."
echo "Register with:"
echo "  tee/scripts/register-enclave.sh \\"
echo "    --use-attestation --expected-pcr0 $PCR0 \\"
echo "    --enclave-url http://<enclave-host>:5000 \\"
echo "    --verifier <TEEMLVerifier-address> \\"
echo "    --rpc-url <rpc-url> --private-key <admin-key>"
