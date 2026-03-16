#!/usr/bin/env bash
# Deploy RemainderVerifier + TEEMLVerifier + DAG circuit to Arbitrum Sepolia.
#
# Required env vars:
#   DEPLOYER_PRIVATE_KEY    -- deployer private key
#   ARBITRUM_SEPOLIA_RPC    -- RPC URL (default: https://sepolia-rollup.arbitrum.io/rpc)
#
# Optional env vars:
#   ARBISCAN_API_KEY        -- for contract verification on Arbiscan
#   REMAINDER_VERIFIER      -- reuse existing RemainderVerifier address
#   SKIP_CIRCUIT_REGISTRATION -- set "true" to skip DAG registration
#
# Usage:
#   DEPLOYER_PRIVATE_KEY=0x... bash scripts/deploy-sepolia.sh
#   DEPLOYER_PRIVATE_KEY=0x... ARBITRUM_SEPOLIA_RPC=http://127.0.0.1:8545 bash scripts/deploy-sepolia.sh
#
# Verify-only mode (check already-deployed contracts exist on chain):
#   bash scripts/deploy-sepolia.sh --verify-only
#   bash scripts/deploy-sepolia.sh --verify-only --deployment-file deployments/anvil-local.json
#
# Local Anvil note:
#   RemainderVerifier has ~129KB bytecode (exceeds EIP-3860 initcode limit) and
#   DAG registration needs ~78M gas (exceeds default 30M block gas limit).
#   Start Anvil with:
#     anvil --gas-limit 500000000 --code-size-limit 200000

set -euo pipefail

# ── Parse CLI flags ──────────────────────────────────────────────────────────
VERIFY_ONLY=false
CUSTOM_DEPLOYMENT_FILE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
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
            echo "Usage: $0 [--verify-only] [--deployment-file <path>]"
            exit 1
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"
DEPLOYMENTS_DIR="$PROJECT_ROOT/deployments"

mkdir -p "$DEPLOYMENTS_DIR"

# ── Verify-only mode ─────────────────────────────────────────────────────────
if [ "$VERIFY_ONLY" = "true" ]; then
    ARBITRUM_SEPOLIA_RPC="${ARBITRUM_SEPOLIA_RPC:-https://sepolia-rollup.arbitrum.io/rpc}"

    # Determine which deployment file to read
    DEPLOY_FILE="${CUSTOM_DEPLOYMENT_FILE:-$DEPLOYMENTS_DIR/arbitrum-sepolia.json}"
    if [ ! -f "$DEPLOY_FILE" ]; then
        echo "ERROR: Deployment file not found: $DEPLOY_FILE"
        echo "Deploy first or specify a file with --deployment-file <path>"
        exit 1
    fi

    echo "=== Verify-only mode ==="
    echo "  Reading: $DEPLOY_FILE"
    echo "  RPC:     $ARBITRUM_SEPOLIA_RPC"
    echo ""

    # Validate JSON
    if ! jq . "$DEPLOY_FILE" > /dev/null 2>&1; then
        echo "ERROR: Invalid JSON in $DEPLOY_FILE"
        exit 1
    fi

    NETWORK=$(jq -r '.network // "unknown"' "$DEPLOY_FILE")
    echo "  Network: $NETWORK"
    echo ""

    PASS=true

    # Check each contract
    for CONTRACT_NAME in $(jq -r '.contracts | keys[]' "$DEPLOY_FILE"); do
        ADDR=$(jq -r ".contracts[\"$CONTRACT_NAME\"]" "$DEPLOY_FILE")
        if [ -z "$ADDR" ] || [ "$ADDR" = "null" ]; then
            echo "  $CONTRACT_NAME: SKIPPED (no address)"
            continue
        fi

        CODE=$(cast code "$ADDR" --rpc-url "$ARBITRUM_SEPOLIA_RPC" 2>/dev/null || echo "0x")
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
        # Update verified flag in deployment file
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

# ── Deploy mode ──────────────────────────────────────────────────────────────

: "${DEPLOYER_PRIVATE_KEY:?Set DEPLOYER_PRIVATE_KEY}"
ARBITRUM_SEPOLIA_RPC="${ARBITRUM_SEPOLIA_RPC:-https://sepolia-rollup.arbitrum.io/rpc}"

echo "=== Deploying to: $ARBITRUM_SEPOLIA_RPC ==="
echo ""

# Build forge arguments
FORGE_ARGS=(
    script script/DeployAll.s.sol:DeployAll
    --rpc-url "$ARBITRUM_SEPOLIA_RPC"
    --private-key "$DEPLOYER_PRIVATE_KEY"
    --broadcast
    --code-size-limit 200000
    -vvv
)

# Add verification flags if API key is set
if [ -n "${ARBISCAN_API_KEY:-}" ]; then
    FORGE_ARGS+=(--verify --etherscan-api-key "$ARBISCAN_API_KEY")
fi

# Forward optional env vars to the forge script
export DEPLOYER_KEY="$DEPLOYER_PRIVATE_KEY"
if [ -n "${REMAINDER_VERIFIER:-}" ]; then
    export REMAINDER_VERIFIER
fi
if [ -n "${SKIP_CIRCUIT_REGISTRATION:-}" ]; then
    export SKIP_CIRCUIT_REGISTRATION
fi

cd "$CONTRACTS_DIR"
forge "${FORGE_ARGS[@]}"

echo ""
echo "=== Extracting deployed addresses ==="

# Detect chain ID from broadcast directory (use most recently modified)
# Arbitrum Sepolia = 421614, local Anvil = 31337
BROADCAST_DIR="$CONTRACTS_DIR/broadcast/DeployAll.s.sol"
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

BROADCAST_FILE="$BROADCAST_DIR/$CHAIN_ID/run-latest.json"

if [ ! -f "$BROADCAST_FILE" ]; then
    echo "ERROR: Broadcast file not found: $BROADCAST_FILE"
    exit 1
fi

# Extract contract addresses from broadcast JSON
# Forge broadcast format: .transactions[] with contractName and contractAddress
REMAINDER_ADDR=$(jq -r '.transactions[] | select(.contractName == "RemainderVerifier") | .contractAddress // empty' "$BROADCAST_FILE" | head -1)
TEE_ADDR=$(jq -r '.transactions[] | select(.contractName == "TEEMLVerifier") | .contractAddress // empty' "$BROADCAST_FILE" | head -1)
DEPLOYER_ADDR=$(jq -r '.transactions[0].transaction.from // empty' "$BROADCAST_FILE")

# If RemainderVerifier was reused (not deployed), use the env var
if [ -z "$REMAINDER_ADDR" ] && [ -n "${REMAINDER_VERIFIER:-}" ]; then
    REMAINDER_ADDR="$REMAINDER_VERIFIER"
fi

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

# Determine network name and deployment file path
NETWORK="arbitrum-sepolia"
if [ "$CHAIN_ID" = "31337" ]; then
    NETWORK="anvil-local"
fi

# Select appropriate deployment file (or use custom if set)
if [ -n "$CUSTOM_DEPLOYMENT_FILE" ]; then
    DEPLOYMENT_FILE="$CUSTOM_DEPLOYMENT_FILE"
else
    DEPLOYMENT_FILE="$DEPLOYMENTS_DIR/${NETWORK}.json"
fi

# Capture timestamp and current block number
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
BLOCK_NUM=$(cast block-number --rpc-url "$ARBITRUM_SEPOLIA_RPC" 2>/dev/null || echo "0")

# Write deployment file with full metadata
cat > "$DEPLOYMENT_FILE" <<EOF
{
  "network": "$NETWORK",
  "chain_id": $CHAIN_ID,
  "deployed_at": "$TIMESTAMP",
  "deployer": "${DEPLOYER_ADDR:-}",
  "block_number": $BLOCK_NUM,
  "contracts": {
    "RemainderVerifier": "${REMAINDER_ADDR:-}",
    "TEEMLVerifier": "${TEE_ADDR:-}"
  },
  "dagCircuit": {
    "registered": $CIRCUIT_REGISTERED,
    "circuitHash": "${CIRCUIT_HASH:-}"
  },
  "circuit_hash": "${CIRCUIT_HASH:-}",
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
echo "=== Deployment Summary ==="
echo "  Network:            $NETWORK (chain $CHAIN_ID)"
echo "  Block:              $BLOCK_NUM"
echo "  Timestamp:          $TIMESTAMP"
echo "  RemainderVerifier:  ${REMAINDER_ADDR:-NOT DEPLOYED}"
echo "  TEEMLVerifier:      ${TEE_ADDR:-NOT DEPLOYED}"
echo "  Circuit hash:       ${CIRCUIT_HASH:-N/A}"
echo "  Deployer:           ${DEPLOYER_ADDR:-}"
echo "  Saved to:           $DEPLOYMENT_FILE"
echo ""

# Also maintain the canonical arbitrum-sepolia.json for backward compatibility
# when deploying to Arbitrum Sepolia (in case someone relied on the fixed path)
CANONICAL_FILE="$DEPLOYMENTS_DIR/arbitrum-sepolia.json"
if [ "$DEPLOYMENT_FILE" != "$CANONICAL_FILE" ] && [ "$NETWORK" = "arbitrum-sepolia" ]; then
    cp "$DEPLOYMENT_FILE" "$CANONICAL_FILE"
    echo "  Also saved to: $CANONICAL_FILE (backward compat)"
fi

echo "=== Done ==="
