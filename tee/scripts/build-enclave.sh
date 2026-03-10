#!/bin/bash
# Build the TEE enclave Docker image and optionally convert to Nitro EIF.
#
# Usage:
#   build-enclave.sh [--nitro] [--output-dir <DIR>]
#
# Flags:
#   --nitro       Also build a Nitro EIF and extract PCR values
#   --output-dir  Directory for EIF and measurements (default: tee/)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

BUILD_NITRO=false
OUTPUT_DIR="$TEE_DIR"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --nitro)      BUILD_NITRO=true; shift ;;
        --output-dir) OUTPUT_DIR="$2"; shift 2 ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

echo "Building TEE enclave Docker image..."
docker build -t tee-enclave "$TEE_DIR"

echo ""
echo "Docker image built: tee-enclave"

if [ "$BUILD_NITRO" = true ]; then
    echo ""
    echo "Building Nitro EIF..."

    EIF_PATH="${OUTPUT_DIR}/tee-enclave.eif"
    MEASUREMENTS_PATH="${OUTPUT_DIR}/enclave-measurements.json"

    # nitro-cli build-enclave outputs JSON with PCR values
    BUILD_OUTPUT=$(nitro-cli build-enclave \
        --docker-uri tee-enclave \
        --output-file "$EIF_PATH" 2>&1)

    echo "$BUILD_OUTPUT"

    # Extract PCR values from nitro-cli output
    # Output format: { "Measurements": { "PCR0": "abc...", "PCR1": "...", "PCR2": "..." } }
    PCR0=$(echo "$BUILD_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['Measurements']['PCR0'])")
    PCR1=$(echo "$BUILD_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['Measurements']['PCR1'])")
    PCR2=$(echo "$BUILD_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['Measurements']['PCR2'])")

    # Save measurements to JSON file
    cat > "$MEASUREMENTS_PATH" <<MEOF
{
  "pcr0": "${PCR0}",
  "pcr1": "${PCR1}",
  "pcr2": "${PCR2}",
  "eif_path": "${EIF_PATH}",
  "built_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
MEOF

    echo ""
    echo "Nitro EIF built: ${EIF_PATH}"
    echo "Measurements saved: ${MEASUREMENTS_PATH}"
    echo ""
    echo "PCR0: ${PCR0}"
    echo "PCR1: ${PCR1}"
    echo "PCR2: ${PCR2}"
else
    echo ""
    echo "To run locally:"
    echo "  docker run -p 8080:8080 -e ENCLAVE_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 tee-enclave"
    echo ""
    echo "To build Nitro EIF:"
    echo "  $0 --nitro"
fi
