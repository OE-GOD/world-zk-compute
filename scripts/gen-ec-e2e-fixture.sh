#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Generate EC Groth16 E2E Fixture
# =============================================================================
#
# Runs the full pipeline: Rust witness → gnark Groth16 proof → merged fixture JSON.
# The output fixture can be loaded by Solidity tests to validate the complete
# hybrid Stylus+Groth16 verification path end-to-end.
#
# Prerequisites:
#   - Rust toolchain with xgboost-remainder crate
#   - gnark-wrapper built (cd examples/xgboost-remainder/gnark-wrapper && make)
#
# Usage:
#   ./scripts/gen-ec-e2e-fixture.sh [OPTIONS]
#
# Options:
#   --trees N       Number of XGBoost trees (default: 3)
#   --depth N       Tree depth (default: 3)
#   --features N    Number of features (default: 4)
#   --output FILE   Output fixture path (default: contracts/test/fixtures/ec_groth16_e2e_fixture.json)
#   --skip-gnark    Skip gnark proof generation (for testing Rust witness only)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RUST_DIR="$PROJECT_ROOT/examples/xgboost-remainder"
GNARK_DIR="$RUST_DIR/gnark-wrapper"

# Default parameters (small model for fast generation)
TREES=3
DEPTH=3
FEATURES=4
OUTPUT="$PROJECT_ROOT/contracts/test/fixtures/ec_groth16_e2e_fixture.json"
SKIP_GNARK=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --trees)     TREES="$2"; shift 2 ;;
        --depth)     DEPTH="$2"; shift 2 ;;
        --features)  FEATURES="$2"; shift 2 ;;
        --output)    OUTPUT="$2"; shift 2 ;;
        --skip-gnark) SKIP_GNARK=true; shift ;;
        *)           echo "Unknown option: $1"; exit 1 ;;
    esac
done

TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

echo "============================================="
echo "  EC Groth16 E2E Fixture Generator"
echo "============================================="
echo ""
echo "  Model: ${TREES} trees, depth ${DEPTH}, ${FEATURES} features"
echo "  Output: ${OUTPUT}"
echo ""

# =============================================================================
# Step 1: Generate EC witness from Rust
# =============================================================================

echo "[Step 1] Generating EC witness (Rust)..."

WITNESS_FILE="$TMPDIR/ec_witness.json"
INNER_PROOF_FILE="$TMPDIR/inner_proof.json"

cd "$RUST_DIR"

# Generate the EC witness (outputs to stdout)
cargo run --release --bin gen_ec_groth16_witness -- \
    --trees "$TREES" --depth "$DEPTH" --features "$FEATURES" \
    > "$WITNESS_FILE" 2>"$TMPDIR/witness_stderr.txt"

if [ ! -s "$WITNESS_FILE" ]; then
    echo "  ERROR: EC witness generation failed"
    cat "$TMPDIR/witness_stderr.txt"
    exit 1
fi

# Extract stats
TOTAL_MUL=$(python3 -c "import json; d=json.load(open('$WITNESS_FILE')); print(d['stats']['totalEcMul'])")
TOTAL_ADD=$(python3 -c "import json; d=json.load(open('$WITNESS_FILE')); print(d['stats']['totalEcAdd'])")
TOTAL_MSM=$(python3 -c "import json; d=json.load(open('$WITNESS_FILE')); print(d['stats'].get('totalMsm', 0))")
TRANSCRIPT_DIGEST=$(python3 -c "import json; d=json.load(open('$WITNESS_FILE')); print(d['transcriptDigest'])")
CIRCUIT_HASH=$(python3 -c "import json; d=json.load(open('$WITNESS_FILE')); print(d['circuitHash'])" 2>/dev/null || echo "N/A")

echo "  EC witness generated: $(wc -c < "$WITNESS_FILE" | tr -d ' ') bytes"
echo "  EC ops: ${TOTAL_MUL} mul, ${TOTAL_ADD} add, ${TOTAL_MSM} MSM"
echo "  Transcript digest: ${TRANSCRIPT_DIGEST:0:18}..."
echo ""

# =============================================================================
# Step 2: Also generate the inner GKR proof fixture for Stylus call
# =============================================================================

echo "[Step 2] Generating inner GKR proof fixture..."

# The gen_ec_groth16_witness also produces the inner proof data we need.
# Extract proof_hex, gens_hex, public_inputs_hex, circuit_hash from the
# phase1a_dag_fixture that corresponds to this model.
# For now, we rely on the existing fixture or generate one.

INNER_FIXTURE="$PROJECT_ROOT/contracts/test/fixtures/phase1a_dag_fixture.json"
if [ -f "$INNER_FIXTURE" ]; then
    echo "  Using existing inner fixture: $(basename "$INNER_FIXTURE")"
    PROOF_HEX=$(python3 -c "import json; d=json.load(open('$INNER_FIXTURE')); print(d['proof_hex'])")
    GENS_HEX=$(python3 -c "import json; d=json.load(open('$INNER_FIXTURE')); print(d['gens_hex'])")
    PUB_INPUTS_HEX=$(python3 -c "import json; d=json.load(open('$INNER_FIXTURE')); print(d['public_inputs_hex'])")
    CIRCUIT_HASH_RAW=$(python3 -c "import json; d=json.load(open('$INNER_FIXTURE')); print(d['circuit_hash_raw'])")
    echo "  Proof size: ${#PROOF_HEX} hex chars"
else
    echo "  WARNING: No inner fixture found at $INNER_FIXTURE"
    echo "  Run gen_phase1a_fixture test first to generate it"
    PROOF_HEX="0x"
    GENS_HEX="0x"
    PUB_INPUTS_HEX="0x"
    CIRCUIT_HASH_RAW="0x0000000000000000000000000000000000000000000000000000000000000000"
fi

echo ""

# =============================================================================
# Step 3: Generate Groth16 proof via gnark-wrapper
# =============================================================================

GROTH16_PROOF_FILE="$TMPDIR/groth16_proof.json"

if [ "$SKIP_GNARK" = false ]; then
    echo "[Step 3] Generating EC Groth16 proof (gnark)..."

    cd "$GNARK_DIR"

    if [ ! -x "./gnark-wrapper" ]; then
        echo "  Building gnark-wrapper..."
        make 2>&1
    fi

    if ./gnark-wrapper help 2>&1 | grep -q "prove-ec-json"; then
        cat "$WITNESS_FILE" | ./gnark-wrapper prove-ec-json --config-json \
            > "$GROTH16_PROOF_FILE" 2>"$TMPDIR/gnark_stderr.txt"

        if [ ! -s "$GROTH16_PROOF_FILE" ]; then
            echo "  ERROR: Groth16 proof generation failed"
            cat "$TMPDIR/gnark_stderr.txt"
            exit 1
        fi

        echo "  Groth16 proof generated: $(wc -c < "$GROTH16_PROOF_FILE" | tr -d ' ') bytes"

        # Extract proof array and public inputs
        GROTH16_PROOF=$(python3 -c "import json; d=json.load(open('$GROTH16_PROOF_FILE')); print(json.dumps(d['proof']))")
        GROTH16_PUB_INPUTS=$(python3 -c "import json; d=json.load(open('$GROTH16_PROOF_FILE')); print(json.dumps(d['public_inputs']))")
        R1CS_CONSTRAINTS=$(python3 -c "import json; d=json.load(open('$GROTH16_PROOF_FILE')); print(d.get('stats',{}).get('r1cs_constraints','N/A'))")

        echo "  R1CS constraints: $R1CS_CONSTRAINTS"
    else
        echo "  WARNING: prove-ec-json not available in gnark-wrapper"
        echo "  Using placeholder proof values"
        GROTH16_PROOF='["0x0","0x0","0x0","0x0","0x0","0x0","0x0","0x0"]'
        GROTH16_PUB_INPUTS="[\"$TRANSCRIPT_DIGEST\",\"0x0\",\"0x0\"]"
    fi
else
    echo "[Step 3] Skipping gnark proof generation (--skip-gnark)"
    GROTH16_PROOF='["0x0","0x0","0x0","0x0","0x0","0x0","0x0","0x0"]'
    GROTH16_PUB_INPUTS="[\"$TRANSCRIPT_DIGEST\",\"0x0\",\"0x0\"]"
fi

echo ""

# =============================================================================
# Step 4: Merge into combined fixture JSON
# =============================================================================

echo "[Step 4] Merging into combined fixture..."

python3 << PYEOF
import json, sys

# Load EC witness
with open("$WITNESS_FILE") as f:
    witness = json.load(f)

# Build combined fixture
fixture = {
    # EC Groth16 proof (8 hex strings -> uint256 values)
    "ecGroth16Proof": json.loads('$GROTH16_PROOF'),
    "ecGroth16PublicInputs": json.loads('$GROTH16_PUB_INPUTS'),

    # Transcript data (from Stylus hybrid output)
    "transcriptDigest": witness["transcriptDigest"],
    "circuitHash": witness.get("circuitHash", "0x0"),

    # Inner GKR proof data (for Stylus call)
    "proof_hex": "$PROOF_HEX",
    "gens_hex": "$GENS_HEX",
    "public_inputs_hex": "$PUB_INPUTS_HEX",
    "circuit_hash_raw": "$CIRCUIT_HASH_RAW",

    # Fr outputs from hybrid verification
    "frOutputs": witness.get("frOutputs", {}),

    # EC operation stats
    "stats": witness.get("stats", {}),

    # Model parameters
    "model": {
        "trees": $TREES,
        "depth": $DEPTH,
        "features": $FEATURES,
    },
}

# Write output
output_path = "$OUTPUT"
with open(output_path, "w") as f:
    json.dump(fixture, f, indent=2)
    f.write("\n")

size = len(json.dumps(fixture))
print(f"  Combined fixture written: {output_path}")
print(f"  Size: {size} bytes")
PYEOF

echo ""

# =============================================================================
# Step 5: Summary
# =============================================================================

echo "============================================="
echo "  Fixture Generation Complete"
echo "============================================="
echo ""
echo "  Output: $OUTPUT"
echo "  Model: ${TREES} trees, depth ${DEPTH}, ${FEATURES} features"
echo "  EC ops: ${TOTAL_MUL} mul, ${TOTAL_ADD} add, ${TOTAL_MSM} MSM"
echo "  Transcript digest: ${TRANSCRIPT_DIGEST:0:18}..."
echo ""
echo "  Use in Forge tests:"
echo "    string memory json = vm.readFile(\"test/fixtures/ec_groth16_e2e_fixture.json\");"
echo "    uint256[8] memory proof = ...parseJsonUintArray(json, \".ecGroth16Proof\");"
echo ""
echo "=== FIXTURE GENERATION PASSED ==="
