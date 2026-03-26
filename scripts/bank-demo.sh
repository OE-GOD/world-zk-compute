#!/usr/bin/env bash
set -euo pipefail

# Bank Demo E2E Script
#
# Runs the full bank credit scoring demo pipeline:
# 1. Build the bank-demo CLI
# 2. Start the verifier API service
# 3. Run inference on sample input
# 4. Submit proof to verifier API
# 5. Archive the result
#
# Usage:
#   ./scripts/bank-demo.sh              # Off-chain mode (default)
#   ./scripts/bank-demo.sh --onchain    # With Anvil for on-chain mode

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

MODEL_PATH="${MODEL_PATH:-examples/bank-demo/models/credit_scoring.json}"
VERIFIER_URL="${VERIFIER_URL:-http://localhost:3000}"
PROOFS_DIR="${PROOFS_DIR:-/tmp/bank-demo-proofs}"
ONCHAIN=false

if [[ "${1:-}" == "--onchain" ]]; then
    ONCHAIN=true
fi

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    docker compose -f docker-compose.bank-demo.yml down 2>/dev/null || true
}
trap cleanup EXIT

cd "$PROJECT_ROOT"

echo "=== Bank Credit Scoring Demo ==="
echo ""

# Step 1: Build
echo "--- Step 1: Building bank-demo CLI ---"
cd examples/bank-demo
cargo build --release 2>&1 | tail -1
cd "$PROJECT_ROOT"
echo "  Built successfully."
echo ""

# Step 2: Start services
echo "--- Step 2: Starting verifier API ---"
if [[ "$ONCHAIN" == true ]]; then
    docker compose -f docker-compose.bank-demo.yml --profile onchain up -d
else
    docker compose -f docker-compose.bank-demo.yml up -d
fi
echo "  Waiting for services..."
for i in $(seq 1 30); do
    if curl -sf "$VERIFIER_URL/health" > /dev/null 2>&1; then
        echo "  Verifier API ready."
        break
    fi
    if [[ $i -eq 30 ]]; then
        echo "  ERROR: Verifier API did not start within 30s"
        exit 1
    fi
    sleep 1
done
echo ""

# Step 3: Run inference
echo "--- Step 3: Running credit scoring inference ---"
INPUT_FILE="examples/bank-demo/sample_input.json"
if [[ -f "$INPUT_FILE" ]]; then
    FEATURES=$(python3 -c "
import json
with open('$INPUT_FILE') as f:
    d = json.load(f)
print(f'--credit-score {d[\"credit_score\"]} --income {d[\"annual_income\"]} --credit-history-years {d[\"credit_history_years\"]} --dti-ratio {d[\"debt_to_income_ratio\"]} --num-accounts {d[\"num_accounts\"]} --recent-inquiries {d[\"recent_inquiries\"]}')
")
else
    FEATURES="--credit-score 720 --income 85000 --credit-history-years 12 --dti-ratio 0.22 --num-accounts 5 --recent-inquiries 1"
fi

RESULT=$(cd examples/bank-demo && cargo run --release -- --model models/credit_scoring.json $FEATURES --json 2>/dev/null)
echo "$RESULT" | python3 -c "
import sys, json
r = json.load(sys.stdin)
print(f'  Decision:    {r[\"decision\"]}')
print(f'  Probability: {r[\"approval_probability\"]*100:.1f}%')
print(f'  Model hash:  0x{r[\"model_hash\"][:16]}...')
print(f'  Input hash:  0x{r[\"input_hash\"][:16]}...')
print(f'  Result hash: 0x{r[\"result_hash\"][:16]}...')
"
echo ""

# Step 4: Check verifier API health
echo "--- Step 4: Verifier API status ---"
HEALTH=$(curl -sf "$VERIFIER_URL/health")
echo "  $HEALTH"
echo ""

# Step 5: Archive result
echo "--- Step 5: Archiving result ---"
mkdir -p "$PROOFS_DIR"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
ARCHIVE_FILE="$PROOFS_DIR/bank-demo-$TIMESTAMP.json"
echo "$RESULT" > "$ARCHIVE_FILE"
echo "  Archived to: $ARCHIVE_FILE"
echo ""

echo "=== Demo Complete ==="
echo ""
echo "Services running:"
echo "  Verifier API: $VERIFIER_URL"
if [[ "$ONCHAIN" == true ]]; then
    echo "  Anvil RPC:    http://localhost:8545"
fi
echo ""
echo "To stop: make demo-bank-down"
