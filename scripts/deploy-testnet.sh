#!/usr/bin/env bash
# Deploy the full verifiable AI stack to a testnet (default: Arbitrum Sepolia).
#
# Required env vars:
#   DEPLOYER_PRIVATE_KEY       -- deployer private key
#
# Optional env vars:
#   ARBITRUM_SEPOLIA_RPC_URL   -- RPC URL (default: https://sepolia-rollup.arbitrum.io/rpc)
#   ARBISCAN_API_KEY           -- for contract verification on Arbiscan
#   FEE_RECIPIENT              -- protocol fee recipient (default: deployer)
#   RISC_ZERO_VERIFIER         -- RISC Zero verifier address (default: deployer placeholder)
#   SKIP_CIRCUIT_REGISTRATION  -- set "true" to skip DAG circuit registration
#   ENCLAVE_KEY                -- TEE enclave key to register
#   ENCLAVE_IMAGE_HASH         -- TEE enclave image hash (requires ENCLAVE_KEY)
#
# Usage:
#   DEPLOYER_PRIVATE_KEY=0x... bash scripts/deploy-testnet.sh
#   DEPLOYER_PRIVATE_KEY=0x... ARBITRUM_SEPOLIA_RPC_URL=http://127.0.0.1:8545 bash scripts/deploy-testnet.sh
#
# Verify-only mode (check deployed contracts exist on chain):
#   bash scripts/deploy-testnet.sh --verify-only
#   bash scripts/deploy-testnet.sh --verify-only --deployment-file deployments/testnet.json
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
    RPC_URL="${ARBITRUM_SEPOLIA_RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}"

    DEPLOY_FILE="${CUSTOM_DEPLOYMENT_FILE:-$DEPLOYMENTS_DIR/testnet.json}"
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
    echo "  Network: $NETWORK"
    echo ""

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

# ── Deploy mode ──────────────────────────────────────────────────────────────

: "${DEPLOYER_PRIVATE_KEY:?Set DEPLOYER_PRIVATE_KEY}"
RPC_URL="${ARBITRUM_SEPOLIA_RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}"

echo "=== Deploying to: $RPC_URL ==="
echo ""

# Build forge arguments
FORGE_ARGS=(
    script script/DeployTestnet.s.sol:DeployTestnet
    --rpc-url "$RPC_URL"
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
export DEPLOYER_PRIVATE_KEY
if [ -n "${FEE_RECIPIENT:-}" ]; then
    export FEE_RECIPIENT
fi
if [ -n "${RISC_ZERO_VERIFIER:-}" ]; then
    export RISC_ZERO_VERIFIER
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

cd "$CONTRACTS_DIR"
forge "${FORGE_ARGS[@]}"

echo ""
echo "=== Extracting deployed addresses ==="

# Detect chain ID from broadcast directory (use most recently modified)
BROADCAST_DIR="$CONTRACTS_DIR/broadcast/DeployTestnet.s.sol"
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
# For proxied contracts, look for UUPSProxy creates and match by order:
#   1st UUPSProxy = RemainderVerifier proxy, 2nd UUPSProxy = TEEMLVerifier proxy
REMAINDER_IMPL=$(jq -r '[.transactions[] | select(.contractName == "RemainderVerifier")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
REMAINDER_PROXY=$(jq -r '[.transactions[] | select(.contractName == "UUPSProxy")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
PROGRAM_REG=$(jq -r '[.transactions[] | select(.contractName == "ProgramRegistry")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
REPUTATION=$(jq -r '[.transactions[] | select(.contractName == "ProverReputation")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
ENGINE=$(jq -r '[.transactions[] | select(.contractName == "ExecutionEngine")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
TEE_IMPL=$(jq -r '[.transactions[] | select(.contractName == "TEEMLVerifier")] | .[0].contractAddress // empty' "$BROADCAST_FILE")
TEE_PROXY=$(jq -r '[.transactions[] | select(.contractName == "UUPSProxy")] | .[1].contractAddress // empty' "$BROADCAST_FILE")
DEPLOYER_ADDR=$(jq -r '.transactions[0].transaction.from // empty' "$BROADCAST_FILE")

# Use proxy addresses for the contracts that are behind UUPS proxies
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

# Determine network name
NETWORK="arbitrum-sepolia"
if [ "$CHAIN_ID" = "31337" ]; then
    NETWORK="anvil-local"
elif [ "$CHAIN_ID" = "4801" ]; then
    NETWORK="world-chain-sepolia"
fi

# Select appropriate deployment file
if [ -n "$CUSTOM_DEPLOYMENT_FILE" ]; then
    DEPLOYMENT_FILE="$CUSTOM_DEPLOYMENT_FILE"
else
    DEPLOYMENT_FILE="$DEPLOYMENTS_DIR/${NETWORK}-testnet.json"
fi

# Capture timestamp and current block number
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
BLOCK_NUM=$(cast block-number --rpc-url "$RPC_URL" 2>/dev/null || echo "0")

# Write deployment file
cat > "$DEPLOYMENT_FILE" <<EOF
{
  "network": "$NETWORK",
  "chain_id": $CHAIN_ID,
  "deployed_at": "$TIMESTAMP",
  "deployer": "${DEPLOYER_ADDR:-}",
  "block_number": $BLOCK_NUM,
  "contracts": {
    "RemainderVerifier": "${REMAINDER_ADDR:-}",
    "RemainderVerifier_impl": "${REMAINDER_IMPL:-}",
    "ProgramRegistry": "${PROGRAM_REG:-}",
    "ProverReputation": "${REPUTATION:-}",
    "ExecutionEngine": "${ENGINE:-}",
    "TEEMLVerifier": "${TEE_ADDR:-}",
    "TEEMLVerifier_impl": "${TEE_IMPL:-}"
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
echo "=== Deployment Summary ==="
echo "  Network:            $NETWORK (chain $CHAIN_ID)"
echo "  Block:              $BLOCK_NUM"
echo "  Timestamp:          $TIMESTAMP"
echo "  Deployer:           ${DEPLOYER_ADDR:-}"
echo ""
echo "  RemainderVerifier:  ${REMAINDER_ADDR:-NOT DEPLOYED}"
echo "  ProgramRegistry:    ${PROGRAM_REG:-NOT DEPLOYED}"
echo "  ProverReputation:   ${REPUTATION:-NOT DEPLOYED}"
echo "  ExecutionEngine:    ${ENGINE:-NOT DEPLOYED}"
echo "  TEEMLVerifier:      ${TEE_ADDR:-NOT DEPLOYED}"
echo ""
echo "  Circuit registered: $CIRCUIT_REGISTERED"
echo "  Circuit hash:       ${CIRCUIT_HASH:-N/A}"
echo "  Saved to:           $DEPLOYMENT_FILE"
echo ""
echo "=== Done ==="
