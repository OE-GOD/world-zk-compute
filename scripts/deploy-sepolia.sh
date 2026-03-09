#!/bin/bash
# Deploy RemainderVerifier + TEEMLVerifier + DAG circuit to Arbitrum Sepolia.
#
# Required env vars:
#   DEPLOYER_PRIVATE_KEY    — deployer private key
#   ARBITRUM_SEPOLIA_RPC    — RPC URL (default: https://sepolia-rollup.arbitrum.io/rpc)
#
# Optional env vars:
#   ARBISCAN_API_KEY        — for contract verification on Arbiscan
#   REMAINDER_VERIFIER      — reuse existing RemainderVerifier address
#   SKIP_CIRCUIT_REGISTRATION — set "true" to skip DAG registration
#
# Usage:
#   DEPLOYER_PRIVATE_KEY=0x... bash scripts/deploy-sepolia.sh
#   DEPLOYER_PRIVATE_KEY=0x... ARBITRUM_SEPOLIA_RPC=http://127.0.0.1:8545 bash scripts/deploy-sepolia.sh

set -euo pipefail

: "${DEPLOYER_PRIVATE_KEY:?Set DEPLOYER_PRIVATE_KEY}"
ARBITRUM_SEPOLIA_RPC="${ARBITRUM_SEPOLIA_RPC:-https://sepolia-rollup.arbitrum.io/rpc}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"
DEPLOYMENTS_DIR="$PROJECT_ROOT/deployments"
DEPLOYMENT_FILE="$DEPLOYMENTS_DIR/arbitrum-sepolia.json"

mkdir -p "$DEPLOYMENTS_DIR"

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

# DAG circuit registration needs ~80M gas (exceeds Anvil default 30M block limit).
# Arbitrum Sepolia supports this natively. For local Anvil, set a high gas limit.
if [[ "$ARBITRUM_SEPOLIA_RPC" == *"127.0.0.1"* ]] || [[ "$ARBITRUM_SEPOLIA_RPC" == *"localhost"* ]]; then
    FORGE_ARGS+=(--gas-limit 500000000)
    echo "(Local RPC detected — using elevated gas limit for DAG registration)"
fi

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

NETWORK="arbitrum-sepolia"
if [ "$CHAIN_ID" = "31337" ]; then
    NETWORK="anvil-local"
fi

TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Write deployment file
cat > "$DEPLOYMENT_FILE" <<EOF
{
  "network": "$NETWORK",
  "chainId": $CHAIN_ID,
  "contracts": {
    "RemainderVerifier": "${REMAINDER_ADDR:-}",
    "TEEMLVerifier": "${TEE_ADDR:-}"
  },
  "dagCircuit": {
    "registered": $CIRCUIT_REGISTERED,
    "circuitHash": "${CIRCUIT_HASH:-}"
  },
  "deployer": "${DEPLOYER_ADDR:-}",
  "deployedAt": "$TIMESTAMP"
}
EOF

echo ""
echo "=== Deployment Summary ==="
echo "  Network:            $NETWORK (chain $CHAIN_ID)"
echo "  RemainderVerifier:  ${REMAINDER_ADDR:-NOT DEPLOYED}"
echo "  TEEMLVerifier:      ${TEE_ADDR:-NOT DEPLOYED}"
echo "  Deployer:           ${DEPLOYER_ADDR:-}"
echo "  Saved to:           $DEPLOYMENT_FILE"
echo ""
echo "=== Done ==="
