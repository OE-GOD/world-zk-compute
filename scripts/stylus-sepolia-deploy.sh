#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Stylus GKR Verifier — Arbitrum Sepolia Deployment
# =============================================================================
#
# Deploys the Stylus GKR verifier to Arbitrum Sepolia testnet, optionally
# deploys the Solidity RemainderVerifier adapter, and tests verification
# via cast call (simulation).
#
# Prerequisites:
#   - cargo-stylus installed (cargo install cargo-stylus)
#   - Foundry installed (forge, cast)
#   - wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown)
#
# Env vars:
#   PRIVATE_KEY        Required. Deployer private key (hex, with 0x prefix).
#   ARB_SEPOLIA_RPC    Optional. Default: https://sepolia-rollup.arbitrum.io/rpc
#
# Usage:
#   PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh
#   PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh --stylus-only
#   PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh --stylus-address 0x...
#   PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh --skip-verify

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VERIFIER_DIR="$PROJECT_ROOT/contracts/stylus/gkr-verifier"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"

RPC_URL="${ARB_SEPOLIA_RPC:-https://sepolia-rollup.arbitrum.io/rpc}"
EXPECTED_CHAIN_ID=421614

# Parse args
STYLUS_ONLY=false
STYLUS_ADDRESS=""
SKIP_VERIFY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --stylus-only)      STYLUS_ONLY=true; shift ;;
        --stylus-address)   STYLUS_ADDRESS="$2"; shift 2 ;;
        --skip-verify)      SKIP_VERIFY=true; shift ;;
        --rpc)              RPC_URL="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: PRIVATE_KEY=0x... $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --stylus-only        Skip Solidity adapter deployment"
            echo "  --stylus-address 0x  Use existing Stylus contract (skip deploy)"
            echo "  --skip-verify        Skip verification simulation"
            echo "  --rpc URL            Override default RPC endpoint"
            echo "  -h, --help           Show this help"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "============================================="
echo "  Stylus GKR Verifier — Arbitrum Sepolia"
echo "============================================="
echo ""

# =============================================================================
# Phase 0: Prerequisites
# =============================================================================

echo "[Phase 0] Checking prerequisites..."

MISSING=false

if ! command -v cargo-stylus &>/dev/null && ! cargo stylus --version &>/dev/null 2>&1; then
    echo "  MISSING: cargo-stylus (install: cargo install cargo-stylus)"
    MISSING=true
fi

if ! command -v forge &>/dev/null; then
    echo "  MISSING: forge (install Foundry: https://getfoundry.sh)"
    MISSING=true
fi

if ! command -v cast &>/dev/null; then
    echo "  MISSING: cast (install Foundry: https://getfoundry.sh)"
    MISSING=true
fi

if ! rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
    echo "  MISSING: wasm32-unknown-unknown target (run: rustup target add wasm32-unknown-unknown)"
    MISSING=true
fi

if [ "$MISSING" = true ]; then
    echo ""
    echo "  Install missing prerequisites and retry."
    exit 1
fi

echo "  All prerequisites found."

# Check for PRIVATE_KEY
if [ -z "${PRIVATE_KEY:-}" ]; then
    echo ""
    echo "============================================="
    echo "  Wallet Setup Required"
    echo "============================================="
    echo ""
    echo "  PRIVATE_KEY env var is not set."
    echo ""
    echo "  To generate a new wallet:"
    echo "    cast wallet new"
    echo ""
    echo "  Then fund it with Arbitrum Sepolia ETH (need ~0.05 ETH):"
    echo "    - Chainlink:   https://faucets.chain.link/arbitrum-sepolia"
    echo "    - Alchemy:     https://www.alchemy.com/faucets/arbitrum-sepolia"
    echo "    - QuickNode:   https://faucet.quicknode.com/arbitrum/sepolia"
    echo ""
    echo "  Then run:"
    echo "    PRIVATE_KEY=0x... $0 $*"
    echo ""
    exit 0
fi

echo ""

# =============================================================================
# Phase 1: Network validation
# =============================================================================

echo "[Phase 1] Validating Arbitrum Sepolia connection..."

CHAIN_ID=$(cast chain-id --rpc-url "$RPC_URL" 2>/dev/null) || {
    echo "  ERROR: Cannot connect to $RPC_URL"
    echo "  Check your network connection or try --rpc with a different endpoint."
    exit 1
}

if [ "$CHAIN_ID" != "$EXPECTED_CHAIN_ID" ]; then
    echo "  ERROR: Expected chain ID $EXPECTED_CHAIN_ID (Arbitrum Sepolia), got $CHAIN_ID"
    exit 1
fi

echo "  Chain ID: $CHAIN_ID (Arbitrum Sepolia)"

DEPLOYER_ADDRESS=$(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null) || {
    echo "  ERROR: Invalid PRIVATE_KEY"
    exit 1
}

BALANCE_WEI=$(cast balance --rpc-url "$RPC_URL" "$DEPLOYER_ADDRESS" 2>/dev/null) || {
    echo "  ERROR: Could not fetch balance for $DEPLOYER_ADDRESS"
    exit 1
}

BALANCE_ETH=$(cast from-wei "$BALANCE_WEI" 2>/dev/null || echo "unknown")
echo "  Deployer: $DEPLOYER_ADDRESS"
echo "  Balance:  $BALANCE_ETH ETH"

# Warn if low balance (< 0.01 ETH = 10000000000000000 wei)
MIN_BALANCE=10000000000000000
if [ "$BALANCE_WEI" != "unknown" ] && [ "$(echo "$BALANCE_WEI" | tr -d '[:space:]')" -lt "$MIN_BALANCE" ] 2>/dev/null; then
    echo ""
    echo "  WARNING: Balance is below 0.01 ETH. Deployment may fail."
    echo "  Fund your wallet at:"
    echo "    - https://faucets.chain.link/arbitrum-sepolia"
    echo "    - https://www.alchemy.com/faucets/arbitrum-sepolia"
    echo ""
fi

echo ""

# =============================================================================
# Phase 2: Build WASM
# =============================================================================

STYLUS_CONTRACT_ADDRESS=""

if [ -z "$STYLUS_ADDRESS" ]; then
    echo "[Phase 2] Building Stylus GKR verifier WASM..."

    cd "$VERIFIER_DIR"

    cargo build --release --lib --target wasm32-unknown-unknown 2>&1 | tail -5

    WASM_FILE="$VERIFIER_DIR/target/wasm32-unknown-unknown/release/gkr_verifier.wasm"
    if [ -f "$WASM_FILE" ]; then
        WASM_SIZE=$(wc -c < "$WASM_FILE" | tr -d ' ')
        echo "  WASM binary: $(echo "scale=1; $WASM_SIZE / 1024" | bc)KB"
    else
        echo "  ERROR: WASM binary not found at $WASM_FILE"
        exit 1
    fi

    echo ""

    # =============================================================================
    # Phase 3: Deploy Stylus contract
    # =============================================================================

    echo "[Phase 3] Deploying Stylus contract to Arbitrum Sepolia..."
    echo "  (This may take 30-60 seconds...)"
    echo ""

    DEPLOY_OUTPUT=$(cargo stylus deploy \
        --endpoint "$RPC_URL" \
        --private-key "$PRIVATE_KEY" \
        --no-verify \
        2>&1) || {
        echo "  ERROR: Stylus deployment failed"
        echo "$DEPLOY_OUTPUT"
        exit 1
    }

    echo "$DEPLOY_OUTPUT"

    # Extract contract address
    STYLUS_CONTRACT_ADDRESS=$(echo "$DEPLOY_OUTPUT" | grep -oE '0x[0-9a-fA-F]{40}' | tail -1)

    if [ -z "$STYLUS_CONTRACT_ADDRESS" ]; then
        echo ""
        echo "  ERROR: Could not extract contract address from deploy output"
        exit 1
    fi

    echo ""
    echo "  Stylus contract deployed: $STYLUS_CONTRACT_ADDRESS"
else
    echo "[Phase 2-3] Using existing Stylus contract: $STYLUS_ADDRESS"
    STYLUS_CONTRACT_ADDRESS="$STYLUS_ADDRESS"
fi

echo ""

# =============================================================================
# Phase 4: Deploy Solidity adapter
# =============================================================================

REMAINDER_VERIFIER_ADDRESS=""

if [ "$STYLUS_ONLY" = false ]; then
    echo "[Phase 4] Deploying Solidity adapter (RemainderVerifier)..."
    echo ""

    cd "$CONTRACTS_DIR"

    FORGE_OUTPUT=$(DEPLOYER_KEY="$PRIVATE_KEY" \
        STYLUS_VERIFIER="$STYLUS_CONTRACT_ADDRESS" \
        forge script script/StylusSepoliaDeploy.s.sol:StylusSepoliaDeploy \
            --rpc-url "$RPC_URL" \
            --broadcast \
            -vvv 2>&1) || {
        echo "  ERROR: Forge script failed"
        echo "$FORGE_OUTPUT"
        exit 1
    }

    echo "$FORGE_OUTPUT"

    # Extract RemainderVerifier address from forge output
    REMAINDER_VERIFIER_ADDRESS=$(echo "$FORGE_OUTPUT" | grep "RemainderVerifier deployed at:" | grep -oE '0x[0-9a-fA-F]{40}' | head -1 || true)

    if [ -z "$REMAINDER_VERIFIER_ADDRESS" ]; then
        echo ""
        echo "  WARNING: Could not extract RemainderVerifier address from output"
    else
        echo ""
        echo "  RemainderVerifier deployed: $REMAINDER_VERIFIER_ADDRESS"
    fi
else
    echo "[Phase 4] Skipping Solidity adapter deployment (--stylus-only)"
fi

echo ""

# =============================================================================
# Phase 5: Verification test (cast call simulation)
# =============================================================================

if [ "$SKIP_VERIFY" = false ] && [ -n "$REMAINDER_VERIFIER_ADDRESS" ]; then
    echo "[Phase 5] Simulating verification via cast call..."
    echo "  (This runs the full ~200M gas verification off-chain via eth_call)"
    echo ""

    cd "$CONTRACTS_DIR"

    # Load fixture data for the cast call
    FIXTURE_FILE="$CONTRACTS_DIR/test/fixtures/phase1a_dag_fixture.json"
    if [ ! -f "$FIXTURE_FILE" ]; then
        echo "  WARNING: Fixture file not found at $FIXTURE_FILE"
        echo "  Skipping verification test."
    else
        # Extract fixture fields for the call
        PROOF_HEX=$(python3 -c "
import json, sys
with open('$FIXTURE_FILE') as f:
    d = json.load(f)
print(d['proof_hex'])
" 2>/dev/null || true)

        CIRCUIT_HASH=$(python3 -c "
import json, sys
with open('$FIXTURE_FILE') as f:
    d = json.load(f)
print(d['circuit_hash_raw'])
" 2>/dev/null || true)

        PUBLIC_INPUTS_HEX=$(python3 -c "
import json, sys
with open('$FIXTURE_FILE') as f:
    d = json.load(f)
print(d['public_inputs_hex'])
" 2>/dev/null || true)

        GENS_HEX=$(python3 -c "
import json, sys
with open('$FIXTURE_FILE') as f:
    d = json.load(f)
print(d['gens_hex'])
" 2>/dev/null || true)

        if [ -n "$PROOF_HEX" ] && [ -n "$CIRCUIT_HASH" ] && [ -n "$PUBLIC_INPUTS_HEX" ] && [ -n "$GENS_HEX" ]; then
            echo "  Calling verifyDAGProofStylus() via eth_call..."
            echo "  (No gas limit on eth_call — full verification will execute)"
            echo ""

            CALL_RESULT=$(cast call \
                --rpc-url "$RPC_URL" \
                "$REMAINDER_VERIFIER_ADDRESS" \
                "verifyDAGProofStylus(bytes,bytes32,bytes,bytes)(bool)" \
                "$PROOF_HEX" \
                "$CIRCUIT_HASH" \
                "$PUBLIC_INPUTS_HEX" \
                "$GENS_HEX" \
                2>&1) || {
                echo "  Verification simulation failed (this may be expected if RPC has call gas limits)"
                echo "  Output: $CALL_RESULT"
                CALL_RESULT=""
            }

            if [ -n "$CALL_RESULT" ]; then
                echo "  Result: $CALL_RESULT"
                if echo "$CALL_RESULT" | grep -qi "true"; then
                    echo "  Verification: PASSED"
                else
                    echo "  Verification: FAILED (result was not true)"
                fi
            fi
        else
            echo "  WARNING: Could not parse fixture file (python3 required)"
            echo "  Skipping verification test."
        fi
    fi
elif [ "$SKIP_VERIFY" = true ]; then
    echo "[Phase 5] Skipping verification simulation (--skip-verify)"
elif [ -z "$REMAINDER_VERIFIER_ADDRESS" ]; then
    echo "[Phase 5] Skipping verification (no RemainderVerifier deployed)"
fi

echo ""

# =============================================================================
# Phase 6: Results summary
# =============================================================================

echo "============================================="
echo "  Deployment Results"
echo "============================================="
echo ""
echo "  Network:          Arbitrum Sepolia (chain ID $EXPECTED_CHAIN_ID)"
echo "  RPC:              $RPC_URL"
echo "  Deployer:         $DEPLOYER_ADDRESS"
echo ""
echo "  Stylus Verifier:  $STYLUS_CONTRACT_ADDRESS"
echo "    Arbiscan: https://sepolia.arbiscan.io/address/$STYLUS_CONTRACT_ADDRESS"

if [ -n "$REMAINDER_VERIFIER_ADDRESS" ]; then
    echo ""
    echo "  RemainderVerifier: $REMAINDER_VERIFIER_ADDRESS"
    echo "    Arbiscan: https://sepolia.arbiscan.io/address/$REMAINDER_VERIFIER_ADDRESS"
fi

echo ""
echo "  Export commands:"
echo "    export STYLUS_VERIFIER=$STYLUS_CONTRACT_ADDRESS"
if [ -n "$REMAINDER_VERIFIER_ADDRESS" ]; then
    echo "    export REMAINDER_VERIFIER=$REMAINDER_VERIFIER_ADDRESS"
fi
echo ""
echo "=== DEPLOYMENT COMPLETE ==="
