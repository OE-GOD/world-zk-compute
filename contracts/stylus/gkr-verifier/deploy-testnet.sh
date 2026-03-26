#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Stylus GKR DAG Verifier — Deploy to Arbitrum Sepolia
# =============================================================================
#
# Builds, optimizes, validates, and deploys the Stylus WASM GKR verifier
# to Arbitrum Sepolia (chain ID 421614).
#
# Prerequisites:
#   - Rust with wasm32-unknown-unknown target  (rustup target add wasm32-unknown-unknown)
#   - cargo-stylus                             (cargo install cargo-stylus)
#   - wasm-opt (binaryen)                      (brew install binaryen / apt install binaryen)
#   - brotli                                   (brew install brotli / apt install brotli)
#   - cast (Foundry)                           (https://getfoundry.sh)
#
# Required environment variables:
#   DEPLOYER_PRIVATE_KEY   Deployer private key (hex, 0x-prefixed)
#
# Optional environment variables:
#   ARB_SEPOLIA_RPC_URL    RPC endpoint (default: https://sepolia-rollup.arbitrum.io/rpc)
#   SKIP_WASM_OPT          Set "true" to skip wasm-opt optimization
#   DRY_RUN                Set "true" to build + validate only, skip deployment
#
# Usage:
#   DEPLOYER_PRIVATE_KEY=0x... bash contracts/stylus/gkr-verifier/deploy-testnet.sh
#   DEPLOYER_PRIVATE_KEY=0x... DRY_RUN=true bash contracts/stylus/gkr-verifier/deploy-testnet.sh
#
# The full deployment pipeline also deploys a Solidity adapter. For that, use:
#   PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh
#
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

RPC_URL="${ARB_SEPOLIA_RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}"
EXPECTED_CHAIN_ID=421614
BROTLI_LIMIT=24576  # 24KB Stylus compressed size limit

WASM_DIR="target/wasm32-unknown-unknown/release"
WASM_RAW="$WASM_DIR/gkr_verifier.wasm"
WASM_OPT="$WASM_DIR/gkr_verifier_opt.wasm"

echo "============================================="
echo "  Stylus GKR Verifier — Arbitrum Sepolia"
echo "============================================="
echo ""

# =============================================================================
# Step 1: Check prerequisites
# =============================================================================

echo "[1/6] Checking prerequisites..."

MISSING=false

if ! command -v cargo &>/dev/null; then
    echo "  MISSING: cargo (install Rust: https://rustup.rs)"
    MISSING=true
fi

if ! cargo stylus --version &>/dev/null 2>&1; then
    echo "  MISSING: cargo-stylus (install: cargo install cargo-stylus)"
    MISSING=true
fi

if ! command -v wasm-opt &>/dev/null; then
    echo "  MISSING: wasm-opt (install: brew install binaryen / apt install binaryen)"
    MISSING=true
fi

if ! command -v brotli &>/dev/null; then
    echo "  MISSING: brotli (install: brew install brotli / apt install brotli)"
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
echo ""

# =============================================================================
# Step 2: Build WASM binary
# =============================================================================

echo "[2/6] Building WASM (release)..."

# The .cargo/config.toml sets the default target to wasm32-unknown-unknown,
# so we use --lib to build the cdylib WASM output.
cargo build --release --lib 2>&1

if [ ! -f "$WASM_RAW" ]; then
    echo "  ERROR: WASM binary not found at $WASM_RAW"
    exit 1
fi

RAW_SIZE=$(wc -c < "$WASM_RAW" | tr -d ' ')
echo "  Raw WASM: $RAW_SIZE bytes ($(echo "scale=1; $RAW_SIZE / 1024" | bc)KB)"
echo ""

# =============================================================================
# Step 3: Optimize with wasm-opt
# =============================================================================

if [ "${SKIP_WASM_OPT:-}" = "true" ]; then
    echo "[3/6] Skipping wasm-opt (SKIP_WASM_OPT=true)"
    # Use the raw binary for deployment
    DEPLOY_WASM="$WASM_RAW"
else
    echo "[3/6] Optimizing with wasm-opt..."
    wasm-opt "$WASM_RAW" -o "$WASM_OPT" --enable-bulk-memory -O3 --zero-filled-memory

    OPT_SIZE=$(wc -c < "$WASM_OPT" | tr -d ' ')
    echo "  Optimized WASM: $OPT_SIZE bytes ($(echo "scale=1; $OPT_SIZE / 1024" | bc)KB)"
    DEPLOY_WASM="$WASM_OPT"
fi

# Check Brotli-compressed size against Stylus limit
BROTLI_SIZE=$(brotli --best --stdout "$DEPLOY_WASM" | wc -c | tr -d ' ')
echo "  Brotli compressed: $BROTLI_SIZE bytes ($(echo "scale=1; $BROTLI_SIZE / 1024" | bc)KB)"
echo "  Stylus limit:      $BROTLI_LIMIT bytes (24.0KB)"

if [ "$BROTLI_SIZE" -gt "$BROTLI_LIMIT" ]; then
    echo ""
    echo "  ERROR: Brotli size ($BROTLI_SIZE) exceeds 24KB Stylus limit ($BROTLI_LIMIT)."
    echo "  The contract is too large to deploy. See docs/STYLUS_DEPLOYMENT.md for tips."
    exit 1
fi

echo "  Size check: PASSED"
echo ""

# =============================================================================
# Step 4: Validate with cargo stylus check
# =============================================================================

echo "[4/6] Validating deployability (cargo stylus check)..."

cargo stylus check --endpoint "$RPC_URL" 2>&1

echo "  Validation: PASSED"
echo ""

# =============================================================================
# Step 5: Check wallet and network
# =============================================================================

echo "[5/6] Checking wallet and network..."

if [ -z "${DEPLOYER_PRIVATE_KEY:-}" ]; then
    echo "  ERROR: DEPLOYER_PRIVATE_KEY is not set."
    echo ""
    echo "  Generate a wallet:   cast wallet new"
    echo "  Fund with Arb Sepolia ETH:"
    echo "    - https://faucets.chain.link/arbitrum-sepolia"
    echo "    - https://www.alchemy.com/faucets/arbitrum-sepolia"
    echo "    - https://faucet.quicknode.com/arbitrum/sepolia"
    echo ""
    echo "  Then run:"
    echo "    DEPLOYER_PRIVATE_KEY=0x... $0"
    exit 1
fi

CHAIN_ID=$(cast chain-id --rpc-url "$RPC_URL" 2>/dev/null) || {
    echo "  ERROR: Cannot connect to $RPC_URL"
    echo "  Check your network connection or set ARB_SEPOLIA_RPC_URL to a different endpoint."
    exit 1
}

if [ "$CHAIN_ID" != "$EXPECTED_CHAIN_ID" ]; then
    echo "  ERROR: Expected chain ID $EXPECTED_CHAIN_ID (Arbitrum Sepolia), got $CHAIN_ID"
    echo "  Set ARB_SEPOLIA_RPC_URL to an Arbitrum Sepolia RPC endpoint."
    exit 1
fi

DEPLOYER_ADDRESS=$(cast wallet address --private-key "$DEPLOYER_PRIVATE_KEY" 2>/dev/null) || {
    echo "  ERROR: Invalid DEPLOYER_PRIVATE_KEY"
    exit 1
}

BALANCE_WEI=$(cast balance --rpc-url "$RPC_URL" "$DEPLOYER_ADDRESS" 2>/dev/null) || {
    echo "  ERROR: Could not fetch balance for $DEPLOYER_ADDRESS"
    exit 1
}

BALANCE_ETH=$(cast from-wei "$BALANCE_WEI" 2>/dev/null || echo "unknown")
echo "  Chain ID:   $CHAIN_ID (Arbitrum Sepolia)"
echo "  Deployer:   $DEPLOYER_ADDRESS"
echo "  Balance:    $BALANCE_ETH ETH"

# Warn if balance < 0.01 ETH
MIN_BALANCE=10000000000000000
BALANCE_CLEAN=$(echo "$BALANCE_WEI" | tr -d '[:space:]')
if [ "$BALANCE_CLEAN" -lt "$MIN_BALANCE" ] 2>/dev/null; then
    echo ""
    echo "  WARNING: Balance is below 0.01 ETH. Deployment may fail."
    echo "  Fund your wallet at https://faucets.chain.link/arbitrum-sepolia"
fi

echo ""

# =============================================================================
# Step 6: Deploy
# =============================================================================

if [ "${DRY_RUN:-}" = "true" ]; then
    echo "[6/6] Dry run mode — skipping deployment."
    echo ""
    echo "  Build and validation succeeded. To deploy for real:"
    echo "    DEPLOYER_PRIVATE_KEY=0x... $0"
    echo ""
    echo "  To deploy the full stack (Stylus + Solidity adapter):"
    echo "    PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh"
    exit 0
fi

echo "[6/6] Deploying Stylus contract to Arbitrum Sepolia..."
echo "  (This may take 30-60 seconds...)"
echo ""

DEPLOY_OUTPUT=$(cargo stylus deploy \
    --endpoint "$RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY" \
    --no-verify \
    2>&1) || {
    echo "  ERROR: Stylus deployment failed."
    echo ""
    echo "$DEPLOY_OUTPUT"
    exit 1
}

echo "$DEPLOY_OUTPUT"
echo ""

# Extract the deployed contract address from cargo stylus output
STYLUS_ADDRESS=$(echo "$DEPLOY_OUTPUT" | grep -oE '0x[0-9a-fA-F]{40}' | tail -1)

if [ -z "$STYLUS_ADDRESS" ]; then
    echo "  WARNING: Could not extract contract address from deploy output."
    echo "  Check the output above for the deployed address."
    exit 1
fi

# =============================================================================
# Summary
# =============================================================================

echo "============================================="
echo "  Deployment Complete"
echo "============================================="
echo ""
echo "  Network:           Arbitrum Sepolia (chain ID $EXPECTED_CHAIN_ID)"
echo "  RPC:               $RPC_URL"
echo "  Deployer:          $DEPLOYER_ADDRESS"
echo "  Stylus Verifier:   $STYLUS_ADDRESS"
echo "  Brotli size:       $BROTLI_SIZE bytes"
echo ""
echo "  Arbiscan: https://sepolia.arbiscan.io/address/$STYLUS_ADDRESS"
echo ""
echo "  Export:"
echo "    export STYLUS_VERIFIER=$STYLUS_ADDRESS"
echo ""
echo "  Next steps:"
echo "    1. Verify contract code on Arbiscan (automatic after ~1 min)"
echo "    2. Deploy the Solidity adapter (RemainderVerifier):"
echo "       PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh --stylus-address $STYLUS_ADDRESS"
echo "    3. Update docs/CONTRACT_ADDRESSES.md with the deployed address"
echo "    4. Set a reactivation reminder for 11 months from now"
echo ""
echo "=== Done ==="
