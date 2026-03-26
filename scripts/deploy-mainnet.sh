#!/usr/bin/env bash
# Deploy the full verifiable AI stack to Arbitrum One mainnet (chainId 42161).
#
# Required env vars:
#   DEPLOYER_PRIVATE_KEY       -- deployer private key
#   ADMIN_ADDRESS              -- multisig/timelock admin address
#   RISC_ZERO_VERIFIER         -- RISC Zero verifier router on Arbitrum One
#   FEE_RECIPIENT              -- protocol fee recipient address
#
# Optional env vars:
#   ARBITRUM_ONE_RPC_URL       -- RPC URL (default: https://arb1.arbitrum.io/rpc)
#   ARBISCAN_API_KEY           -- for contract verification on Arbiscan
#   STAKING_TOKEN              -- ERC-20 staking token address
#   MIN_STAKE                  -- minimum prover stake in wei (default: 100e18)
#   SLASH_BPS                  -- slash basis points (default: 500 = 5%)
#   SKIP_CIRCUIT_REGISTRATION  -- set "true" to skip DAG circuit registration
#   ENCLAVE_KEY                -- TEE enclave key to register
#   ENCLAVE_IMAGE_HASH         -- TEE enclave image hash (requires ENCLAVE_KEY)
#
# Usage:
#   DEPLOYER_PRIVATE_KEY=0x... ADMIN_ADDRESS=0x... RISC_ZERO_VERIFIER=0x... \
#     FEE_RECIPIENT=0x... bash scripts/deploy-mainnet.sh
#
# Dry-run mode (simulation only, no broadcast):
#   DEPLOYER_PRIVATE_KEY=0x... ADMIN_ADDRESS=0x... RISC_ZERO_VERIFIER=0x... \
#     FEE_RECIPIENT=0x... bash scripts/deploy-mainnet.sh --dry-run
#
# Verify-only mode (check deployed contracts exist on chain):
#   bash scripts/deploy-mainnet.sh --verify-only
#   bash scripts/deploy-mainnet.sh --verify-only --deployment-file deployments/arbitrum-mainnet.json

set -euo pipefail

# ── Parse CLI flags ──────────────────────────────────────────────────────────
DRY_RUN=false
VERIFY_ONLY=false
CUSTOM_DEPLOYMENT_FILE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --verify-only)
            VERIFY_ONLY=true
            shift
            ;;
        --deployment-file)
            CUSTOM_DEPLOYMENT_FILE="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--dry-run] [--verify-only] [--deployment-file <path>]"
            exit 1
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"
DEPLOYMENTS_DIR="$PROJECT_ROOT/deployments"

mkdir -p "$DEPLOYMENTS_DIR"

EXPECTED_CHAIN_ID=42161
RPC_URL="${ARBITRUM_ONE_RPC_URL:-https://arb1.arbitrum.io/rpc}"

# ── Verify-only mode ─────────────────────────────────────────────────────────
if [ "$VERIFY_ONLY" = "true" ]; then
    DEPLOY_FILE="${CUSTOM_DEPLOYMENT_FILE:-$DEPLOYMENTS_DIR/arbitrum-mainnet.json}"
    if [ ! -f "$DEPLOY_FILE" ]; then
        echo "ERROR: Deployment file not found: $DEPLOY_FILE"
        echo "Deploy first or specify a file with --deployment-file <path>"
        exit 1
    fi

    echo "=== Verify-only mode ==="
    echo "  Reading: $DEPLOY_FILE"
    echo "  RPC:     $RPC_URL"
    echo ""

    if ! jq . "$DEPLOY_FILE" > /dev/null 2>&1; then
        echo "ERROR: Invalid JSON in $DEPLOY_FILE"
        exit 1
    fi

    NETWORK=$(jq -r '.network // "unknown"' "$DEPLOY_FILE")
    CHAIN_ID=$(jq -r '.chain_id // "unknown"' "$DEPLOY_FILE")
    echo "  Network:  $NETWORK"
    echo "  Chain ID: $CHAIN_ID"
    echo ""

    if [ "$CHAIN_ID" != "$EXPECTED_CHAIN_ID" ]; then
        echo "WARNING: Deployment file chain ID ($CHAIN_ID) does not match expected ($EXPECTED_CHAIN_ID)"
    fi

    PASS=true

    for CONTRACT_NAME in $(jq -r '.contracts | keys[]' "$DEPLOY_FILE"); do
        ADDR=$(jq -r ".contracts[\"$CONTRACT_NAME\"]" "$DEPLOY_FILE")
        if [ -z "$ADDR" ] || [ "$ADDR" = "null" ]; then
            echo "  $CONTRACT_NAME: SKIPPED (no address)"
            continue
        fi

        CODE=$(cast code "$ADDR" --rpc-url "$RPC_URL" 2>/dev/null || echo "0x")
        if [ "$CODE" = "0x" ] || [ -z "$CODE" ]; then
            echo "  $CONTRACT_NAME ($ADDR): FAIL (no code on chain)"
            PASS=false
        else
            CODE_LEN=${#CODE}
            echo "  $CONTRACT_NAME ($ADDR): OK (code size: $((CODE_LEN / 2 - 1)) bytes)"
        fi
    done

    echo ""
    if [ "$PASS" = "true" ]; then
        echo "=== All contracts verified ==="
        TMP_FILE=$(mktemp)
        jq '.verified = true' "$DEPLOY_FILE" > "$TMP_FILE" && mv "$TMP_FILE" "$DEPLOY_FILE"
        echo "  Updated 'verified' flag to true in $DEPLOY_FILE"
    else
        echo "=== Some contracts FAILED verification ==="
        TMP_FILE=$(mktemp)
        jq '.verified = false' "$DEPLOY_FILE" > "$TMP_FILE" && mv "$TMP_FILE" "$DEPLOY_FILE"
        exit 1
    fi
    exit 0
fi

# ── Validate required env vars ───────────────────────────────────────────────
: "${DEPLOYER_PRIVATE_KEY:?Set DEPLOYER_PRIVATE_KEY}"
: "${ADMIN_ADDRESS:?Set ADMIN_ADDRESS (multisig/timelock)}"
: "${RISC_ZERO_VERIFIER:?Set RISC_ZERO_VERIFIER (verifier router address)}"
: "${FEE_RECIPIENT:?Set FEE_RECIPIENT (protocol fee recipient)}"

# ── Safety confirmation ──────────────────────────────────────────────────────
echo ""
echo "=========================================="
echo "  ARBITRUM ONE MAINNET DEPLOYMENT"
echo "=========================================="
echo ""
echo "  RPC URL:            $RPC_URL"
echo "  Admin (owner):      $ADMIN_ADDRESS"
echo "  RISC Zero Verifier: $RISC_ZERO_VERIFIER"
echo "  Fee Recipient:      $FEE_RECIPIENT"
echo "  Staking Token:      ${STAKING_TOKEN:-NONE}"
echo "  Dry Run:            $DRY_RUN"
echo ""

if [ "$DRY_RUN" = "false" ]; then
    echo "WARNING: This is a MAINNET deployment. Transactions will be broadcast."
    echo ""
    read -r -p "Type 'deploy' to confirm: " CONFIRM
    if [ "$CONFIRM" != "deploy" ]; then
        echo "Aborted."
        exit 1
    fi
    echo ""
fi

# ── Build forge arguments ────────────────────────────────────────────────────
FORGE_ARGS=(
    script script/DeployMainnet.s.sol:DeployMainnet
    --rpc-url "$RPC_URL"
    --private-key "$DEPLOYER_PRIVATE_KEY"
    --code-size-limit 200000
    -vvv
    --slow
)

if [ "$DRY_RUN" = "false" ]; then
    FORGE_ARGS+=(--broadcast)
fi

# Add verification flags if API key is set
if [ -n "${ARBISCAN_API_KEY:-}" ]; then
    FORGE_ARGS+=(--verify --etherscan-api-key "$ARBISCAN_API_KEY")
fi

# ── Forward env vars to the forge script ─────────────────────────────────────
export DEPLOYER_PRIVATE_KEY
export ADMIN_ADDRESS
export RISC_ZERO_VERIFIER
export FEE_RECIPIENT

if [ -n "${STAKING_TOKEN:-}" ]; then
    export STAKING_TOKEN
fi
if [ -n "${MIN_STAKE:-}" ]; then
    export MIN_STAKE
fi
if [ -n "${SLASH_BPS:-}" ]; then
    export SLASH_BPS
fi
if [ -n "${SKIP_CIRCUIT_REGISTRATION:-}" ]; then
    export SKIP_CIRCUIT_REGISTRATION
fi
if [ -n "${ENCLAVE_KEY:-}" ]; then
    export ENCLAVE_KEY
fi
if [ -n "${ENCLAVE_IMAGE_HASH:-}" ]; then
    export ENCLAVE_IMAGE_HASH
fi

# ── Run forge script ─────────────────────────────────────────────────────────
cd "$CONTRACTS_DIR"
echo "Running: forge ${FORGE_ARGS[*]}"
echo ""
forge "${FORGE_ARGS[@]}"

if [ "$DRY_RUN" = "true" ]; then
    echo ""
    echo "=== DRY RUN COMPLETE ==="
    echo "  Simulation succeeded. No transactions were broadcast."
    echo "  Remove --dry-run to deploy for real."
    exit 0
fi

# ── Extract deployed addresses ───────────────────────────────────────────────
echo ""
echo "=== Extracting deployed addresses ==="

BROADCAST_DIR="$CONTRACTS_DIR/broadcast/DeployMainnet.s.sol"
CHAIN_ID=""
LATEST_TIME=0
for d in "$BROADCAST_DIR"/*/; do
    [ -d "$d" ] || continue
    RUN_FILE="$d/run-latest.json"
    [ -f "$RUN_FILE" ] || continue
    MOD_TIME=$(stat -f %m "$RUN_FILE" 2>/dev/null || stat -c %Y "$RUN_FILE" 2>/dev/null || echo 0)
    if [ "$MOD_TIME" -gt "$LATEST_TIME" ]; then
        LATEST_TIME=$MOD_TIME
        CHAIN_ID="$(basename "$d")"
    fi
done

if [ -z "$CHAIN_ID" ]; then
    echo "ERROR: No broadcast directory found. Deploy may have failed."
    exit 1
fi

if [ "$CHAIN_ID" != "$EXPECTED_CHAIN_ID" ]; then
    echo "WARNING: Detected chain ID $CHAIN_ID, expected $EXPECTED_CHAIN_ID"
fi

BROADCAST_FILE="$BROADCAST_DIR/$CHAIN_ID/run-latest.json"

if [ ! -f "$BROADCAST_FILE" ]; then
    echo "ERROR: Broadcast file not found: $BROADCAST_FILE"
    exit 1
fi

# Extract contract addresses from broadcast JSON
REMAINDER_IMPL=$(jq -r '[.transactions[] | select(.contractName == "RemainderVerifier")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
REMAINDER_PROXY=$(jq -r '[.transactions[] | select(.contractName == "UUPSProxy")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
PROGRAM_REG=$(jq -r '[.transactions[] | select(.contractName == "ProgramRegistry")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
REPUTATION=$(jq -r '[.transactions[] | select(.contractName == "ProverReputation")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
PROVER_REG=$(jq -r '[.transactions[] | select(.contractName == "ProverRegistry")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
ENGINE=$(jq -r '[.transactions[] | select(.contractName == "ExecutionEngine")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
TEE_IMPL=$(jq -r '[.transactions[] | select(.contractName == "TEEMLVerifier")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
TEE_PROXY=$(jq -r '[.transactions[] | select(.contractName == "UUPSProxy")] | .[1].contractAddress // empty' "$BROADCAST_FILE")
DEPLOYER_ADDR=$(jq -r '.transactions[0].transaction.from // empty' "$BROADCAST_FILE")

# Use proxy addresses for contracts behind UUPS proxies
REMAINDER_ADDR="${REMAINDER_PROXY:-$REMAINDER_IMPL}"
TEE_ADDR="${TEE_PROXY:-$TEE_IMPL}"

# Extract circuit hash from fixture (if circuit was registered)
CIRCUIT_REGISTERED=true
CIRCUIT_HASH=""
if [ "${SKIP_CIRCUIT_REGISTRATION:-}" = "true" ]; then
    CIRCUIT_REGISTERED=false
else
    FIXTURE_FILE="$CONTRACTS_DIR/test/fixtures/phase1a_dag_fixture.json"
    if [ -f "$FIXTURE_FILE" ]; then
        CIRCUIT_HASH=$(jq -r '.circuit_hash_raw // empty' "$FIXTURE_FILE")
    fi
fi

# Select deployment file
if [ -n "$CUSTOM_DEPLOYMENT_FILE" ]; then
    DEPLOYMENT_FILE="$CUSTOM_DEPLOYMENT_FILE"
else
    DEPLOYMENT_FILE="$DEPLOYMENTS_DIR/arbitrum-mainnet.json"
fi

# Capture timestamp and current block number
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
BLOCK_NUM=$(cast block-number --rpc-url "$RPC_URL" 2>/dev/null || echo "0")

# Write deployment file
cat > "$DEPLOYMENT_FILE" <<EOF
{
  "network": "arbitrum-one",
  "chain_id": $CHAIN_ID,
  "deployed_at": "$TIMESTAMP",
  "deployer": "${DEPLOYER_ADDR:-}",
  "admin": "$ADMIN_ADDRESS",
  "block_number": $BLOCK_NUM,
  "contracts": {
    "RemainderVerifier": "${REMAINDER_ADDR:-}",
    "RemainderVerifier_impl": "${REMAINDER_IMPL:-}",
    "ProgramRegistry": "${PROGRAM_REG:-}",
    "ProverReputation": "${REPUTATION:-}",
    "ProverRegistry": "${PROVER_REG:-}",
    "ExecutionEngine": "${ENGINE:-}",
    "TEEMLVerifier": "${TEE_ADDR:-}",
    "TEEMLVerifier_impl": "${TEE_IMPL:-}"
  },
  "config": {
    "riscZeroVerifier": "$RISC_ZERO_VERIFIER",
    "feeRecipient": "$FEE_RECIPIENT",
    "stakingToken": "${STAKING_TOKEN:-}",
    "minStake": "${MIN_STAKE:-100000000000000000000}",
    "slashBps": "${SLASH_BPS:-500}"
  },
  "dagCircuit": {
    "registered": $CIRCUIT_REGISTERED,
    "circuitHash": "${CIRCUIT_HASH:-}"
  },
  "verified": false
}
EOF

# Validate generated JSON
if ! jq . "$DEPLOYMENT_FILE" > /dev/null 2>&1; then
    echo "ERROR: Generated invalid JSON in $DEPLOYMENT_FILE"
    cat "$DEPLOYMENT_FILE"
    exit 1
fi

echo ""
echo "=========================================="
echo "  DEPLOYMENT SUMMARY"
echo "=========================================="
echo ""
echo "  Network:            arbitrum-one (chain $CHAIN_ID)"
echo "  Block:              $BLOCK_NUM"
echo "  Timestamp:          $TIMESTAMP"
echo "  Deployer:           ${DEPLOYER_ADDR:-}"
echo "  Admin:              $ADMIN_ADDRESS"
echo ""
echo "  RemainderVerifier:  ${REMAINDER_ADDR:-NOT DEPLOYED}"
echo "  ProgramRegistry:    ${PROGRAM_REG:-NOT DEPLOYED}"
echo "  ProverReputation:   ${REPUTATION:-NOT DEPLOYED}"
echo "  ProverRegistry:     ${PROVER_REG:-NOT DEPLOYED}"
echo "  ExecutionEngine:    ${ENGINE:-NOT DEPLOYED}"
echo "  TEEMLVerifier:      ${TEE_ADDR:-NOT DEPLOYED}"
echo ""
echo "  Circuit registered: $CIRCUIT_REGISTERED"
echo "  Circuit hash:       ${CIRCUIT_HASH:-N/A}"
echo "  Saved to:           $DEPLOYMENT_FILE"
echo ""
echo "=========================================="
echo "  POST-DEPLOYMENT CHECKLIST"
echo "=========================================="
echo ""
echo "  1. Admin ($ADMIN_ADDRESS) calls acceptOwnership() on:"
echo "     - ProgramRegistry  (${PROGRAM_REG:-TBD})"
echo "     - ProverReputation (${REPUTATION:-TBD})"
echo "     - ExecutionEngine  (${ENGINE:-TBD})"
if [ -n "${PROVER_REG:-}" ]; then
echo "     - ProverRegistry   (${PROVER_REG})"
fi
echo ""
echo "  2. Verify admin/owner on all contracts:"
echo "     cast call <addr> 'admin()' --rpc-url $RPC_URL"
echo "     cast call <addr> 'owner()' --rpc-url $RPC_URL"
echo ""
echo "  3. Verify contract wiring:"
echo "     cast call ${TEE_ADDR:-TBD} 'remainderVerifier()' --rpc-url $RPC_URL"
echo "     cast call ${ENGINE:-TBD} 'feeRecipient()' --rpc-url $RPC_URL"
echo ""
echo "  4. Set timelock on UUPS contracts:"
echo "     cast send <RemainderVerifier> 'setTimelock(address)' <TIMELOCK_ADDR>"
echo "     cast send <TEEMLVerifier> 'setTimelock(address)' <TIMELOCK_ADDR>"
echo ""
echo "  5. Register ML programs and TEE enclaves"
echo "  6. Unpause contracts when ready for operations"
echo "  7. Verify deployment: bash scripts/deploy-mainnet.sh --verify-only"
echo ""
echo "=== Done ==="
