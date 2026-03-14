#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# Multi-chain deployment script for World ZK Compute contracts.
#
# Reads chain configurations from deployments/chains.json and deploys
# contracts to one or all chains.
#
# Required env vars:
#   DEPLOYER_PRIVATE_KEY    -- deployer private key
#
# Optional env vars:
#   REMAINDER_VERIFIER      -- reuse existing RemainderVerifier address
#   SKIP_CIRCUIT_REGISTRATION -- set "true" to skip DAG registration
#
# Usage:
#   DEPLOYER_PRIVATE_KEY=0x... bash scripts/deploy-multichain.sh
#   DEPLOYER_PRIVATE_KEY=0x... bash scripts/deploy-multichain.sh --chain arbitrum-sepolia
#   DEPLOYER_PRIVATE_KEY=0x... bash scripts/deploy-multichain.sh --chain localhost --dry-run
#   bash scripts/deploy-multichain.sh --list
#   bash scripts/deploy-multichain.sh --help
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"
DEPLOYMENTS_DIR="$PROJECT_ROOT/deployments"
CHAINS_FILE="$DEPLOYMENTS_DIR/chains.json"

# ── CLI flags ────────────────────────────────────────────────────────────────
TARGET_CHAIN=""
VERIFY_FLAG=false
DRY_RUN=false
LIST_ONLY=false

usage() {
    cat <<'USAGE'
deploy-multichain.sh — Deploy contracts to one or all configured chains.

USAGE:
  scripts/deploy-multichain.sh [OPTIONS]

OPTIONS:
  --chain NAME    Deploy to a specific chain only (e.g., arbitrum-sepolia)
  --verify        Verify contracts on block explorer after deployment
  --dry-run       Simulate deployment without broadcasting transactions
  --list          List all configured chains and exit
  --help          Show this help message

ENVIRONMENT:
  DEPLOYER_PRIVATE_KEY    (required) Private key for deployment
  REMAINDER_VERIFIER      (optional) Reuse existing RemainderVerifier address
  SKIP_CIRCUIT_REGISTRATION (optional) Set "true" to skip DAG registration
  <EXPLORER>_API_KEY      (optional) API key for block explorer verification

EXAMPLES:
  DEPLOYER_PRIVATE_KEY=0x... ./scripts/deploy-multichain.sh --chain localhost --dry-run
  DEPLOYER_PRIVATE_KEY=0x... ./scripts/deploy-multichain.sh --chain arbitrum-sepolia --verify
  DEPLOYER_PRIVATE_KEY=0x... ./scripts/deploy-multichain.sh  # deploy to ALL chains
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --chain)
            TARGET_CHAIN="$2"
            shift 2
            ;;
        --verify)
            VERIFY_FLAG=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --list)
            LIST_ONLY=true
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# ── Validate prerequisites ──────────────────────────────────────────────────
if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required. Install with: brew install jq"
    exit 1
fi

if ! command -v forge &>/dev/null; then
    echo "ERROR: forge is required. Install with: curl -L https://foundry.paradigm.xyz | bash"
    exit 1
fi

if [ ! -f "$CHAINS_FILE" ]; then
    echo "ERROR: Chain config not found: $CHAINS_FILE"
    exit 1
fi

# ── List mode ────────────────────────────────────────────────────────────────
if [ "$LIST_ONLY" = "true" ]; then
    echo "Configured chains:"
    echo ""
    jq -r '.chains[] | "  \(.name) (chain \(.chainId))\n    RPC: \(.rpcUrl)\n    Explorer: \(.explorerUrl)\n    Notes: \(.notes)\n"' "$CHAINS_FILE"
    exit 0
fi

# ── Validate deployer key ────────────────────────────────────────────────────
: "${DEPLOYER_PRIVATE_KEY:?Set DEPLOYER_PRIVATE_KEY}"

# ── Build chain list ────────────────────────────────────────────────────────
CHAIN_COUNT=$(jq '.chains | length' "$CHAINS_FILE")

if [ -n "$TARGET_CHAIN" ]; then
    # Validate target chain exists
    FOUND=$(jq -r --arg name "$TARGET_CHAIN" '.chains[] | select(.name == $name) | .name' "$CHAINS_FILE")
    if [ -z "$FOUND" ]; then
        echo "ERROR: Chain '$TARGET_CHAIN' not found in $CHAINS_FILE"
        echo "Available chains:"
        jq -r '.chains[].name' "$CHAINS_FILE" | sed 's/^/  /'
        exit 1
    fi
fi

# ── Deploy function ──────────────────────────────────────────────────────────
deploy_to_chain() {
    local chain_name="$1"
    local chain_id rpc_url explorer_url explorer_api_key_env code_size_limit gas_limit

    chain_id=$(jq -r --arg n "$chain_name" '.chains[] | select(.name == $n) | .chainId' "$CHAINS_FILE")
    rpc_url=$(jq -r --arg n "$chain_name" '.chains[] | select(.name == $n) | .rpcUrl' "$CHAINS_FILE")
    explorer_url=$(jq -r --arg n "$chain_name" '.chains[] | select(.name == $n) | .explorerUrl' "$CHAINS_FILE")
    explorer_api_key_env=$(jq -r --arg n "$chain_name" '.chains[] | select(.name == $n) | .explorerApiKey' "$CHAINS_FILE")
    code_size_limit=$(jq -r --arg n "$chain_name" '.chains[] | select(.name == $n) | .codeSizeLimit' "$CHAINS_FILE")
    # shellcheck disable=SC2034
    gas_limit=$(jq -r --arg n "$chain_name" '.chains[] | select(.name == $n) | .gasLimit' "$CHAINS_FILE")

    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  Deploying to: $chain_name (chain $chain_id)"
    echo "  RPC:          $rpc_url"
    echo "  Explorer:     ${explorer_url:-none}"
    if [ "$DRY_RUN" = "true" ]; then
        echo "  Mode:         DRY RUN (no broadcast)"
    fi
    echo "═══════════════════════════════════════════════════════════════"

    # Build forge arguments
    local forge_args=(
        script script/DeployAll.s.sol:DeployAll
        --rpc-url "$rpc_url"
        --private-key "$DEPLOYER_PRIVATE_KEY"
        --code-size-limit "$code_size_limit"
        -vvv
    )

    if [ "$DRY_RUN" = "false" ]; then
        forge_args+=(--broadcast)
    fi

    # Add verification flags if requested and API key is available
    if [ "$VERIFY_FLAG" = "true" ] && [ -n "$explorer_api_key_env" ]; then
        local api_key="${!explorer_api_key_env:-}"
        if [ -n "$api_key" ]; then
            forge_args+=(--verify --etherscan-api-key "$api_key")
            echo "  Verification: enabled (using $explorer_api_key_env)"
        else
            echo "  Verification: skipped ($explorer_api_key_env not set)"
        fi
    fi

    # Forward optional env vars
    export DEPLOYER_KEY="$DEPLOYER_PRIVATE_KEY"
    if [ -n "${REMAINDER_VERIFIER:-}" ]; then
        export REMAINDER_VERIFIER
    fi
    if [ -n "${SKIP_CIRCUIT_REGISTRATION:-}" ]; then
        export SKIP_CIRCUIT_REGISTRATION
    fi

    # Run forge deploy
    cd "$CONTRACTS_DIR"
    if ! forge "${forge_args[@]}"; then
        echo ""
        echo "  FAILED: Deployment to $chain_name failed"
        return 1
    fi

    if [ "$DRY_RUN" = "true" ]; then
        echo ""
        echo "  DRY RUN complete for $chain_name — no transactions broadcast"
        return 0
    fi

    # ── Extract deployed addresses ───────────────────────────────────────
    echo ""
    echo "  Extracting deployed addresses..."

    local broadcast_dir="$CONTRACTS_DIR/broadcast/DeployAll.s.sol/$chain_id"
    local broadcast_file="$broadcast_dir/run-latest.json"

    if [ ! -f "$broadcast_file" ]; then
        echo "  WARNING: Broadcast file not found: $broadcast_file"
        return 0
    fi

    local remainder_addr tee_addr deployer_addr
    remainder_addr=$(jq -r '.transactions[] | select(.contractName == "RemainderVerifier") | .contractAddress // empty' "$broadcast_file" | head -1)
    tee_addr=$(jq -r '.transactions[] | select(.contractName == "TEEMLVerifier") | .contractAddress // empty' "$broadcast_file" | head -1)
    deployer_addr=$(jq -r '.transactions[0].transaction.from // empty' "$broadcast_file")

    # If RemainderVerifier was reused
    if [ -z "$remainder_addr" ] && [ -n "${REMAINDER_VERIFIER:-}" ]; then
        remainder_addr="$REMAINDER_VERIFIER"
    fi

    # Extract circuit hash
    local circuit_registered=true circuit_hash=""
    if [ "${SKIP_CIRCUIT_REGISTRATION:-}" = "true" ]; then
        circuit_registered=false
    else
        local fixture_file="$CONTRACTS_DIR/test/fixtures/phase1a_dag_fixture.json"
        if [ -f "$fixture_file" ]; then
            circuit_hash=$(jq -r '.circuit_hash_raw // empty' "$fixture_file")
        fi
    fi

    local timestamp block_num
    timestamp=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    block_num=$(cast block-number --rpc-url "$rpc_url" 2>/dev/null || echo "0")

    # Write deployment file
    local deploy_file="$DEPLOYMENTS_DIR/${chain_name}.json"
    cat > "$deploy_file" <<EOF
{
  "network": "$chain_name",
  "chain_id": $chain_id,
  "deployed_at": "$timestamp",
  "deployer": "${deployer_addr:-}",
  "block_number": $block_num,
  "contracts": {
    "RemainderVerifier": "${remainder_addr:-}",
    "TEEMLVerifier": "${tee_addr:-}"
  },
  "dagCircuit": {
    "registered": $circuit_registered,
    "circuitHash": "${circuit_hash:-}"
  },
  "verified": false
}
EOF

    echo ""
    echo "  Deployment saved to: $deploy_file"
    echo "  RemainderVerifier:  ${remainder_addr:-NOT DEPLOYED}"
    echo "  TEEMLVerifier:      ${tee_addr:-NOT DEPLOYED}"
    echo "  Block:              $block_num"
    echo ""
    echo "  SUCCESS: $chain_name deployment complete"
}

# ── Main ─────────────────────────────────────────────────────────────────────
mkdir -p "$DEPLOYMENTS_DIR"

DEPLOY_PASS=0
DEPLOY_FAIL=0
DEPLOY_RESULTS=()

if [ -n "$TARGET_CHAIN" ]; then
    # Deploy to single chain
    if deploy_to_chain "$TARGET_CHAIN"; then
        DEPLOY_PASS=$((DEPLOY_PASS + 1))
        DEPLOY_RESULTS+=("PASS  $TARGET_CHAIN")
    else
        DEPLOY_FAIL=$((DEPLOY_FAIL + 1))
        DEPLOY_RESULTS+=("FAIL  $TARGET_CHAIN")
    fi
else
    # Deploy to all chains
    for i in $(seq 0 $((CHAIN_COUNT - 1))); do
        chain_name=$(jq -r ".chains[$i].name" "$CHAINS_FILE")
        if deploy_to_chain "$chain_name"; then
            DEPLOY_PASS=$((DEPLOY_PASS + 1))
            DEPLOY_RESULTS+=("PASS  $chain_name")
        else
            DEPLOY_FAIL=$((DEPLOY_FAIL + 1))
            DEPLOY_RESULTS+=("FAIL  $chain_name")
        fi
    done
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  DEPLOYMENT SUMMARY"
echo "═══════════════════════════════════════════════════════════════"
for result in "${DEPLOY_RESULTS[@]}"; do
    echo "  $result"
done
echo "---------------------------------------------------------------"
echo "  Passed: $DEPLOY_PASS | Failed: $DEPLOY_FAIL"
echo "═══════════════════════════════════════════════════════════════"

if [ "$DEPLOY_FAIL" -gt 0 ]; then
    exit 1
fi
