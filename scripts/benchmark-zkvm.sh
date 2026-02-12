#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# World ZK Compute - zkVM Benchmark Comparison
#
# Runs the same guest program on multiple zkVMs and compares:
#   - Cycle count (execute mode)
#   - Proof generation time
#   - Proof size
#   - Verification result
#
# Usage:
#   ./scripts/benchmark-zkvm.sh anomaly-detector              # Execute only
#   ./scripts/benchmark-zkvm.sh anomaly-detector --prove       # Include proof
#   ./scripts/benchmark-zkvm.sh rule-engine                    # Execute only
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

EXAMPLE="${1:-}"
PROVE=false

if [[ -z "$EXAMPLE" ]]; then
    echo "Usage: $0 <example-name> [--prove]"
    echo ""
    echo "Available examples:"
    echo "  anomaly-detector    Statistical z-score anomaly detection"
    echo "  rule-engine         Configurable rule evaluation engine"
    echo ""
    echo "Options:"
    echo "  --prove             Also generate and verify proofs (slow)"
    exit 1
fi

shift
while [[ $# -gt 0 ]]; do
    case "$1" in
        --prove) PROVE=true; shift ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ── Check prerequisites ──────────────────────────────────────────────────────

check_risc0() {
    if ! command -v cargo &>/dev/null; then
        echo "ERROR: cargo not found"
        return 1
    fi
    return 0
}

check_sp1() {
    if ! command -v cargo-prove &>/dev/null; then
        echo "WARNING: SP1 toolchain not installed. Install with:"
        echo "  curl -L https://sp1.succinct.xyz | bash && sp1up"
        return 1
    fi
    return 0
}

# ── Run RISC Zero benchmark ──────────────────────────────────────────────────

run_risczero() {
    local example="$1"
    local risc0_dir="$ROOT_DIR/examples/$example"

    if [[ ! -d "$risc0_dir" ]]; then
        echo "SKIP: RISC Zero example not found at $risc0_dir"
        return 1
    fi

    # Check for workspace Cargo.toml (needed for host)
    if [[ ! -f "$risc0_dir/Cargo.toml" ]]; then
        echo "SKIP: No workspace Cargo.toml at $risc0_dir"
        return 1
    fi

    echo "Building and running RISC Zero variant..."
    cd "$risc0_dir"

    local output
    output=$(cargo run --release --bin "${example}-host" 2>&1) || true
    echo "$output"

    # Parse metrics from output (macOS-compatible)
    RISC0_CYCLES=$(echo "$output" | sed -n 's/.*RISC0 Cycles: \([0-9]*\).*/\1/p' | head -1)
    RISC0_CYCLES="${RISC0_CYCLES:-N/A}"
    RISC0_EXEC_TIME=$(echo "$output" | sed -n 's/.*RISC0 Execution time: \(.*\)/\1/p' | head -1)
    RISC0_EXEC_TIME="${RISC0_EXEC_TIME:-N/A}"
}

# ── Run SP1 benchmark ────────────────────────────────────────────────────────

run_sp1() {
    local example="$1"
    local prove="$2"
    local sp1_dir="$ROOT_DIR/examples/${example}-sp1"

    if [[ ! -d "$sp1_dir" ]]; then
        echo "SKIP: SP1 example not found at $sp1_dir"
        return 1
    fi

    echo "Building and running SP1 variant..."
    cd "$sp1_dir"

    local args="--execute"
    if [[ "$prove" == "true" ]]; then
        args="--execute --prove"
    fi

    local output
    output=$(cargo run --release -p "${example}-sp1-script" -- $args 2>&1) || true
    echo "$output"

    # Parse metrics from output (macOS-compatible — no grep -P)
    SP1_CYCLES=$(echo "$output" | sed -n 's/.*SP1 Cycles: \([0-9]*\).*/\1/p' | head -1)
    SP1_CYCLES="${SP1_CYCLES:-N/A}"
    SP1_EXEC_TIME=$(echo "$output" | sed -n 's/.*SP1 Execution time: \(.*\)/\1/p' | head -1)
    SP1_EXEC_TIME="${SP1_EXEC_TIME:-N/A}"
    SP1_PROOF_TIME=$(echo "$output" | sed -n 's/.*SP1 Proof time: \(.*\)/\1/p' | head -1)
    SP1_PROOF_TIME="${SP1_PROOF_TIME:-N/A}"
    SP1_PROOF_SIZE=$(echo "$output" | sed -n 's/.*SP1 Proof size: \([0-9]* bytes\).*/\1/p' | head -1)
    SP1_PROOF_SIZE="${SP1_PROOF_SIZE:-N/A}"
    SP1_VERIFIED=$(echo "$output" | grep -q 'verified' && echo "PASS" || echo "N/A")
}

# ── Main ──────────────────────────────────────────────────────────────────────

echo "═══════════════════════════════════════════════════════════════"
echo "  zkVM Benchmark: $EXAMPLE"
echo "═══════════════════════════════════════════════════════════════"
echo ""

# Initialize metrics
RISC0_CYCLES="N/A"
RISC0_EXEC_TIME="N/A"
SP1_CYCLES="N/A"
SP1_EXEC_TIME="N/A"
SP1_PROOF_TIME="N/A"
SP1_PROOF_SIZE="N/A"
SP1_VERIFIED="N/A"

RISC0_AVAILABLE=false
SP1_AVAILABLE=false

# Check availability
if check_risc0; then
    RISC0_AVAILABLE=true
fi

if check_sp1; then
    SP1_AVAILABLE=true
fi

echo ""

# Run RISC Zero
if [[ "$RISC0_AVAILABLE" == "true" ]]; then
    echo "──────────────────────────────────────────────────────────────"
    echo "  RISC Zero"
    echo "──────────────────────────────────────────────────────────────"
    run_risczero "$EXAMPLE" || true
    echo ""
fi

# Run SP1
if [[ "$SP1_AVAILABLE" == "true" ]]; then
    echo "──────────────────────────────────────────────────────────────"
    echo "  SP1"
    echo "──────────────────────────────────────────────────────────────"
    run_sp1 "$EXAMPLE" "$PROVE" || true
    echo ""
fi

# Print comparison table
echo "══════════════════════════════════════════════════════════════════"
echo "  Benchmark Results: $EXAMPLE"
echo "══════════════════════════════════════════════════════════════════"
echo ""
printf "| %-22s | %-20s | %-20s |\n" "Metric" "RISC Zero" "SP1"
printf "|%-24s|%-22s|%-22s|\n" "------------------------" "----------------------" "----------------------"
printf "| %-22s | %-20s | %-20s |\n" "Cycle count" "$RISC0_CYCLES" "$SP1_CYCLES"
printf "| %-22s | %-20s | %-20s |\n" "Execution time" "$RISC0_EXEC_TIME" "$SP1_EXEC_TIME"

if [[ "$PROVE" == "true" ]]; then
    printf "| %-22s | %-20s | %-20s |\n" "Proof time" "N/A" "$SP1_PROOF_TIME"
    printf "| %-22s | %-20s | %-20s |\n" "Proof size" "N/A" "$SP1_PROOF_SIZE"
    printf "| %-22s | %-20s | %-20s |\n" "Verification" "N/A" "$SP1_VERIFIED"
fi
echo ""
