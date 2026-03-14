#!/usr/bin/env bash
# =============================================================================
# Unified Deployment Script for World ZK Compute
#
# Deploys contracts in order using forge create, saves addresses to
# deployments/<chain-id>.json, validates each deployment, and optionally
# verifies on Etherscan/Blockscout.
#
# Usage:
#   DEPLOYER_PRIVATE_KEY=0x... ./scripts/deploy.sh --chain local
#   DEPLOYER_PRIVATE_KEY=0x... ./scripts/deploy.sh --chain sepolia --verify
#   DEPLOYER_PRIVATE_KEY=0x... ./scripts/deploy.sh --chain arbitrum-sepolia --dry-run
#   DEPLOYER_PRIVATE_KEY=0x... ./scripts/deploy.sh --chain sepolia --with-remainder
#   ./scripts/deploy.sh --chain local --private-key 0xac0974bec...
#   ./scripts/deploy.sh --help
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Color helpers (respects NO_COLOR)
# ---------------------------------------------------------------------------
if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' CYAN='' BOLD='' RESET=''
fi

info()    { printf "${CYAN}[INFO]${RESET}  %s\n" "$*"; }
ok()      { printf "${GREEN}[OK]${RESET}    %s\n" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[ERROR]${RESET} %s\n" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"
DEPLOYMENTS_DIR="$PROJECT_ROOT/deployments"
CHAINS_FILE="$DEPLOYMENTS_DIR/chains.json"

# ---------------------------------------------------------------------------
# Default flags
# ---------------------------------------------------------------------------
CHAIN=""
VERIFY_FLAG=false
DRY_RUN=false
WITH_REMAINDER=false
PRIVATE_KEY="${DEPLOYER_PRIVATE_KEY:-}"
FEE_RECIPIENT="${FEE_RECIPIENT:-}"
REMAINDER_VERIFIER_ADDR="${REMAINDER_VERIFIER:-0x0000000000000000000000000000000000000000}"

# ---------------------------------------------------------------------------
# Tracking arrays for rollback and summary
# Using indexed counters for bash 3.2 compatibility (macOS)
# ---------------------------------------------------------------------------
DEPLOY_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

# Parallel arrays for deployed contracts
# DEPLOYED_NAME_0, DEPLOYED_ADDR_0, DEPLOYED_TX_0, etc.
set_deployed() {
    eval "DEPLOYED_NAME_${DEPLOY_COUNT}=\"$1\""
    eval "DEPLOYED_ADDR_${DEPLOY_COUNT}=\"$2\""
    eval "DEPLOYED_TX_${DEPLOY_COUNT}=\"$3\""
    DEPLOY_COUNT=$((DEPLOY_COUNT + 1))
}

get_deployed_name() { eval "echo \"\${DEPLOYED_NAME_${1}}\""; }
get_deployed_addr() { eval "echo \"\${DEPLOYED_ADDR_${1}}\""; }
get_deployed_tx()   { eval "echo \"\${DEPLOYED_TX_${1}}\""; }

set_failed() {
    eval "FAILED_NAME_${FAIL_COUNT}=\"$1\""
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

get_failed_name() { eval "echo \"\${FAILED_NAME_${1}}\""; }

set_skipped() {
    eval "SKIPPED_NAME_${SKIP_COUNT}=\"$1\""
    SKIP_COUNT=$((SKIP_COUNT + 1))
}

get_skipped_name() { eval "echo \"\${SKIPPED_NAME_${1}}\""; }

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
deploy.sh -- Unified deployment script for World ZK Compute contracts.

USAGE:
  scripts/deploy.sh --chain <chain> [OPTIONS]

REQUIRED:
  --chain <name>         Target chain: local | sepolia | mainnet | arbitrum-sepolia
                         (must exist in deployments/chains.json)

OPTIONS:
  --verify               Verify contracts on Etherscan/Blockscout after deployment
  --dry-run              Simulate deployment (no transactions broadcast)
  --with-remainder       Also deploy RemainderVerifier
  --private-key <key>    Deployer private key (alternative to DEPLOYER_PRIVATE_KEY env)
  --fee-recipient <addr> Fee recipient address (default: deployer address)
  --help                 Show this help message

ENVIRONMENT:
  DEPLOYER_PRIVATE_KEY   Private key for deployment (if --private-key not given)
  FEE_RECIPIENT          Fee recipient address (overridden by --fee-recipient)
  REMAINDER_VERIFIER     Existing RemainderVerifier address to reuse (skips deploy)

DEPLOY ORDER:
  1. MockRiscZeroVerifier (for local) or uses existing verifier router
  2. ProgramRegistry(admin)
  3. TEEMLVerifier(admin, remainderVerifier)
  4. ExecutionEngine(admin, programRegistry, verifier, feeRecipient)
  5. [optional] RemainderVerifier(admin)

OUTPUT:
  Deployed addresses saved to deployments/<chain-id>.json

EXAMPLES:
  DEPLOYER_PRIVATE_KEY=0xac09... ./scripts/deploy.sh --chain local
  DEPLOYER_PRIVATE_KEY=0xac09... ./scripts/deploy.sh --chain sepolia --verify
  DEPLOYER_PRIVATE_KEY=0xac09... ./scripts/deploy.sh --chain local --dry-run
  ./scripts/deploy.sh --chain local --private-key 0xac09... --with-remainder
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --chain)
            CHAIN="$2"
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
        --with-remainder)
            WITH_REMAINDER=true
            shift
            ;;
        --private-key)
            PRIVATE_KEY="$2"
            shift 2
            ;;
        --fee-recipient)
            FEE_RECIPIENT="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            err "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Chain name aliases (user-friendly -> chains.json name)
# ---------------------------------------------------------------------------
resolve_chain_alias() {
    case "$1" in
        local) echo "localhost" ;;
        *)     echo "$1" ;;
    esac
}
CHAIN=$(resolve_chain_alias "$CHAIN")

# ---------------------------------------------------------------------------
# Validate prerequisites
# ---------------------------------------------------------------------------
if [[ -z "$CHAIN" ]]; then
    err "--chain is required. Choose: local, sepolia, mainnet, arbitrum-sepolia"
    echo ""
    usage
    exit 1
fi

if ! command -v forge &>/dev/null; then
    err "forge is required. Install: curl -L https://foundry.paradigm.xyz | bash"
    exit 1
fi

if ! command -v cast &>/dev/null; then
    err "cast is required. Install: curl -L https://foundry.paradigm.xyz | bash"
    exit 1
fi

if ! command -v jq &>/dev/null; then
    err "jq is required. Install: brew install jq"
    exit 1
fi

if [[ ! -f "$CHAINS_FILE" ]]; then
    err "Chain config not found: $CHAINS_FILE"
    exit 1
fi

# ---------------------------------------------------------------------------
# Read chain config from deployments/chains.json
# ---------------------------------------------------------------------------
CHAIN_JSON=$(jq -r --arg name "$CHAIN" '.chains[] | select(.name == $name)' "$CHAINS_FILE")

if [[ -z "$CHAIN_JSON" || "$CHAIN_JSON" == "null" ]]; then
    err "Chain '$CHAIN' not found in $CHAINS_FILE"
    echo "Available chains:"
    jq -r '.chains[].name' "$CHAINS_FILE" | sed 's/^/  /'
    exit 1
fi

CHAIN_ID=$(echo "$CHAIN_JSON" | jq -r '.chainId')
RPC_URL=$(echo "$CHAIN_JSON" | jq -r '.rpcUrl')
EXPLORER_URL=$(echo "$CHAIN_JSON" | jq -r '.explorerUrl // empty')
EXPLORER_API_KEY_ENV=$(echo "$CHAIN_JSON" | jq -r '.explorerApiKey // empty')
CODE_SIZE_LIMIT=$(echo "$CHAIN_JSON" | jq -r '.codeSizeLimit // 24576')
GAS_LIMIT=$(echo "$CHAIN_JSON" | jq -r '.gasLimit // 30000000')
VERIFIER_ROUTER=$(echo "$CHAIN_JSON" | jq -r '.verifierRouter // empty')

# ---------------------------------------------------------------------------
# Validate private key
# ---------------------------------------------------------------------------
if [[ -z "$PRIVATE_KEY" ]]; then
    err "Deployer private key required. Set DEPLOYER_PRIVATE_KEY or use --private-key."
    exit 1
fi

# Derive deployer address from private key
DEPLOYER_ADDR=$(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null) || {
    err "Invalid private key. Could not derive address."
    exit 1
}

# Default fee recipient to deployer
if [[ -z "$FEE_RECIPIENT" ]]; then
    FEE_RECIPIENT="$DEPLOYER_ADDR"
fi

# ---------------------------------------------------------------------------
# Deployment output file
# ---------------------------------------------------------------------------
DEPLOY_FILE="$DEPLOYMENTS_DIR/${CHAIN_ID}.json"
mkdir -p "$DEPLOYMENTS_DIR"

# ---------------------------------------------------------------------------
# Print deployment plan
# ---------------------------------------------------------------------------
header "============================================================"
header "  World ZK Compute -- Unified Deployment"
header "============================================================"
echo ""
info "Chain:            $CHAIN (id: $CHAIN_ID)"
info "RPC:              $RPC_URL"
info "Deployer:         $DEPLOYER_ADDR"
info "Fee Recipient:    $FEE_RECIPIENT"
info "Code Size Limit:  $CODE_SIZE_LIMIT"
info "Gas Limit:        $GAS_LIMIT"

if [[ -n "$VERIFIER_ROUTER" ]]; then
    info "Verifier Router:  $VERIFIER_ROUTER"
fi

if [[ "$WITH_REMAINDER" == "true" ]]; then
    info "RemainderVerifier: will deploy"
elif [[ "$REMAINDER_VERIFIER_ADDR" != "0x0000000000000000000000000000000000000000" ]]; then
    info "RemainderVerifier: reusing $REMAINDER_VERIFIER_ADDR"
fi

if [[ "$VERIFY_FLAG" == "true" ]]; then
    info "Verification:     enabled"
fi

if [[ "$DRY_RUN" == "true" ]]; then
    warn "DRY RUN mode -- no transactions will be broadcast"
fi

echo ""

# ---------------------------------------------------------------------------
# Check deployer balance (skip for dry-run on local)
# ---------------------------------------------------------------------------
check_balance() {
    local balance_wei
    balance_wei=$(cast balance "$DEPLOYER_ADDR" --rpc-url "$RPC_URL" 2>/dev/null) || {
        warn "Could not check deployer balance (RPC may be down)"
        return 0
    }
    local balance_eth
    balance_eth=$(cast from-wei "$balance_wei" 2>/dev/null || echo "unknown")
    info "Deployer balance: $balance_eth ETH"

    # Warn if balance looks too low (< 0.01 ETH)
    if [[ "$balance_wei" != "unknown" ]] && [[ "$balance_wei" -lt 10000000000000000 ]] 2>/dev/null; then
        warn "Deployer balance is low. Deployment may fail due to insufficient funds."
    fi
}

if [[ "$DRY_RUN" == "false" ]]; then
    check_balance
fi

# ---------------------------------------------------------------------------
# Build contracts first
# ---------------------------------------------------------------------------
header "Building contracts..."
cd "$CONTRACTS_DIR"
if ! forge build --force 2>&1; then
    err "forge build failed. Fix compilation errors before deploying."
    exit 1
fi
ok "Contracts compiled successfully"
echo ""

# ---------------------------------------------------------------------------
# Deploy helper function
# ---------------------------------------------------------------------------
# Deploys a contract and captures its address and tx hash.
# Usage: deploy_contract <name> <source_path:contract_name> [constructor_args...]
# Sets: LAST_DEPLOY_ADDR, LAST_DEPLOY_TX
LAST_DEPLOY_ADDR=""
LAST_DEPLOY_TX=""

deploy_contract() {
    local name="$1"
    local contract="$2"
    shift 2

    info "Deploying $name..."

    if [[ "$DRY_RUN" == "true" ]]; then
        warn "  [DRY RUN] Would deploy $contract with args: ${*:-none}"
        LAST_DEPLOY_ADDR="0xDRY_RUN_$(printf '%036x' $RANDOM)"
        LAST_DEPLOY_TX="0xDRY_RUN_TX_$(printf '%052x' $RANDOM)"
        set_deployed "$name" "$LAST_DEPLOY_ADDR" "$LAST_DEPLOY_TX"
        ok "  [DRY RUN] $name would be deployed"
        return 0
    fi

    # Build forge create command
    local forge_args="forge create $contract --rpc-url $RPC_URL --private-key $PRIVATE_KEY --json"

    # Add constructor args if any
    if [[ $# -gt 0 ]]; then
        forge_args="$forge_args --constructor-args $*"
    fi

    # Execute deployment
    local output
    output=$(eval "$forge_args" 2>&1) || {
        err "  Failed to deploy $name"
        err "  Output: $output"
        set_failed "$name"
        return 1
    }

    # Parse JSON output for deployed address and tx hash
    LAST_DEPLOY_ADDR=$(echo "$output" | jq -r '.deployedTo // empty' 2>/dev/null)
    LAST_DEPLOY_TX=$(echo "$output" | jq -r '.transactionHash // empty' 2>/dev/null)

    if [[ -z "$LAST_DEPLOY_ADDR" || "$LAST_DEPLOY_ADDR" == "null" ]]; then
        err "  Failed to parse deployed address for $name"
        err "  Raw output: $output"
        set_failed "$name"
        return 1
    fi

    set_deployed "$name" "$LAST_DEPLOY_ADDR" "$LAST_DEPLOY_TX"

    ok "  $name deployed at: $LAST_DEPLOY_ADDR"
    info "  TX: $LAST_DEPLOY_TX"
    return 0
}

# ---------------------------------------------------------------------------
# Post-deploy validation helper
# ---------------------------------------------------------------------------
validate_contract() {
    local name="$1"
    local addr="$2"
    local func_sig="$3"  # e.g., "owner()(address)"

    if [[ "$DRY_RUN" == "true" ]]; then
        ok "  [DRY RUN] Would validate $name at $addr"
        return 0
    fi

    info "  Validating $name at $addr..."
    local result
    result=$(cast call "$addr" "$func_sig" --rpc-url "$RPC_URL" 2>&1) || {
        err "  Validation FAILED for $name: contract not responding"
        err "  Output: $result"
        return 1
    }
    ok "  $name responds: $func_sig => $result"
    return 0
}

# ---------------------------------------------------------------------------
# Verify helper function
# ---------------------------------------------------------------------------
verify_contract() {
    local name="$1"
    local addr="$2"
    local contract="$3"
    shift 3

    if [[ "$VERIFY_FLAG" == "false" ]]; then
        return 0
    fi

    if [[ -z "$EXPLORER_API_KEY_ENV" ]]; then
        warn "  No explorer API key configured for $CHAIN. Skipping verification."
        return 0
    fi

    local api_key
    eval "api_key=\"\${${EXPLORER_API_KEY_ENV}:-}\""
    if [[ -z "$api_key" ]]; then
        warn "  $EXPLORER_API_KEY_ENV not set. Skipping verification of $name."
        return 0
    fi

    info "  Verifying $name on block explorer..."

    local verify_cmd="forge verify-contract $addr $contract --chain-id $CHAIN_ID --etherscan-api-key $api_key"

    if [[ $# -gt 0 ]]; then
        local encoded_args
        encoded_args=$(cast abi-encode "constructor($1)" "${@:2}" 2>/dev/null) || true
        if [[ -n "${encoded_args:-}" ]]; then
            verify_cmd="$verify_cmd --constructor-args $encoded_args"
        fi
    fi

    if eval "$verify_cmd" 2>&1; then
        ok "  $name verified on $EXPLORER_URL"
    else
        warn "  Verification of $name failed (contract may already be verified or API error)"
    fi
}

# ---------------------------------------------------------------------------
# Save progress to deployment file after each contract
# ---------------------------------------------------------------------------
save_deployment() {
    local timestamp
    timestamp=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    # Build the contracts JSON object
    local contracts_json="{"
    local first=true
    local idx=0
    while [[ $idx -lt $DEPLOY_COUNT ]]; do
        local d_name d_addr d_tx
        d_name=$(get_deployed_name $idx)
        d_addr=$(get_deployed_addr $idx)
        d_tx=$(get_deployed_tx $idx)
        if [[ "$first" == "true" ]]; then
            first=false
        else
            contracts_json+=","
        fi
        contracts_json+="\"$d_name\": {\"address\": \"$d_addr\", \"txHash\": \"$d_tx\"}"
        idx=$((idx + 1))
    done
    contracts_json+="}"

    # Build failed contracts array
    local failed_json="["
    local first_fail=true
    idx=0
    while [[ $idx -lt $FAIL_COUNT ]]; do
        local f_name
        f_name=$(get_failed_name $idx)
        if [[ "$first_fail" == "true" ]]; then
            first_fail=false
        else
            failed_json+=","
        fi
        failed_json+="\"$f_name\""
        idx=$((idx + 1))
    done
    failed_json+="]"

    cat > "$DEPLOY_FILE" <<EOF
{
  "network": "$CHAIN",
  "chainId": $CHAIN_ID,
  "deployedAt": "$timestamp",
  "deployer": "$DEPLOYER_ADDR",
  "feeRecipient": "$FEE_RECIPIENT",
  "dryRun": $DRY_RUN,
  "contracts": $contracts_json,
  "failed": $failed_json
}
EOF
}

# ---------------------------------------------------------------------------
# Rollback info printer (called on failure)
# ---------------------------------------------------------------------------
print_rollback_info() {
    header "============================================================"
    printf "  %sDEPLOYMENT INCOMPLETE -- Rollback Info%s\n" "$RED" "$RESET"
    header "============================================================"
    echo ""

    if [[ $DEPLOY_COUNT -gt 0 ]]; then
        warn "The following contracts were deployed successfully:"
        local idx=0
        while [[ $idx -lt $DEPLOY_COUNT ]]; do
            printf "  ${GREEN}%-25s${RESET} %s\n" "$(get_deployed_name $idx)" "$(get_deployed_addr $idx)"
            idx=$((idx + 1))
        done
        echo ""
        warn "These contracts are on-chain and cannot be automatically rolled back."
        warn "If they are proxies, you can upgrade them. Otherwise, abandon and redeploy."
    fi

    if [[ $FAIL_COUNT -gt 0 ]]; then
        echo ""
        err "The following contracts FAILED to deploy:"
        local idx=0
        while [[ $idx -lt $FAIL_COUNT ]]; do
            printf "  ${RED}%-25s${RESET} NOT DEPLOYED\n" "$(get_failed_name $idx)"
            idx=$((idx + 1))
        done
    fi

    if [[ $SKIP_COUNT -gt 0 ]]; then
        echo ""
        warn "The following contracts were SKIPPED (due to earlier failure):"
        local idx=0
        while [[ $idx -lt $SKIP_COUNT ]]; do
            printf "  ${YELLOW}%-25s${RESET} SKIPPED\n" "$(get_skipped_name $idx)"
            idx=$((idx + 1))
        done
    fi

    echo ""
    info "Partial deployment saved to: $DEPLOY_FILE"
}

# ---------------------------------------------------------------------------
# Main deployment sequence
# ---------------------------------------------------------------------------
DEPLOY_FAILED=false

# Track addresses for wiring
MOCK_VERIFIER_ADDR=""
PROGRAM_REGISTRY_ADDR=""
TEE_VERIFIER_ADDR=""
EXECUTION_ENGINE_ADDR=""
REMAINDER_VERIFIER_DEPLOYED_ADDR=""

header "Step 1/5: Deploy Verifier"
# For local chains, deploy MockRiscZeroVerifier. For live chains, use
# verifierRouter from config or deploy a mock as fallback.
if [[ -n "$VERIFIER_ROUTER" && "$VERIFIER_ROUTER" != "null" ]]; then
    info "Using existing verifier router: $VERIFIER_ROUTER"
    MOCK_VERIFIER_ADDR="$VERIFIER_ROUTER"
    set_deployed "VerifierRouter (existing)" "$VERIFIER_ROUTER" "n/a (pre-existing)"
elif [[ "$CHAIN" == "local" || "$CHAIN" == "localhost" ]]; then
    if deploy_contract "MockRiscZeroVerifier" "src/MockRiscZeroVerifier.sol:MockRiscZeroVerifier"; then
        MOCK_VERIFIER_ADDR="$LAST_DEPLOY_ADDR"
        save_deployment
    else
        DEPLOY_FAILED=true
    fi
else
    # Non-local chain without verifier router: deploy mock with warning
    warn "No verifierRouter configured for '$CHAIN'. Deploying MockRiscZeroVerifier."
    warn "For production, set verifierRouter in $CHAINS_FILE."
    if deploy_contract "MockRiscZeroVerifier" "src/MockRiscZeroVerifier.sol:MockRiscZeroVerifier"; then
        MOCK_VERIFIER_ADDR="$LAST_DEPLOY_ADDR"
        save_deployment
    else
        DEPLOY_FAILED=true
    fi
fi

# Abort remaining steps if the verifier failed
if [[ "$DEPLOY_FAILED" == "true" ]]; then
    set_skipped "ProgramRegistry"
    set_skipped "TEEMLVerifier"
    set_skipped "ExecutionEngine"
    if [[ "$WITH_REMAINDER" == "true" ]]; then
        set_skipped "RemainderVerifier"
    fi
    save_deployment
    print_rollback_info
    exit 1
fi

header "Step 2/5: Deploy ProgramRegistry"
if deploy_contract "ProgramRegistry" "src/ProgramRegistry.sol:ProgramRegistry" "$DEPLOYER_ADDR"; then
    PROGRAM_REGISTRY_ADDR="$LAST_DEPLOY_ADDR"
    validate_contract "ProgramRegistry" "$PROGRAM_REGISTRY_ADDR" "owner()(address)" || true
    verify_contract "ProgramRegistry" "$PROGRAM_REGISTRY_ADDR" "src/ProgramRegistry.sol:ProgramRegistry" \
        "address" "$DEPLOYER_ADDR"
    save_deployment
else
    DEPLOY_FAILED=true
    set_skipped "TEEMLVerifier"
    set_skipped "ExecutionEngine"
    if [[ "$WITH_REMAINDER" == "true" ]]; then
        set_skipped "RemainderVerifier"
    fi
    save_deployment
    print_rollback_info
    exit 1
fi

header "Step 3/5: Deploy TEEMLVerifier"
# TEEMLVerifier(admin, remainderVerifier)
TEE_REMAINDER_ARG="$REMAINDER_VERIFIER_ADDR"
if deploy_contract "TEEMLVerifier" "src/tee/TEEMLVerifier.sol:TEEMLVerifier" "$DEPLOYER_ADDR" "$TEE_REMAINDER_ARG"; then
    TEE_VERIFIER_ADDR="$LAST_DEPLOY_ADDR"
    validate_contract "TEEMLVerifier" "$TEE_VERIFIER_ADDR" "owner()(address)" || true
    verify_contract "TEEMLVerifier" "$TEE_VERIFIER_ADDR" "src/tee/TEEMLVerifier.sol:TEEMLVerifier" \
        "address,address" "$DEPLOYER_ADDR" "$TEE_REMAINDER_ARG"
    save_deployment
else
    DEPLOY_FAILED=true
    set_skipped "ExecutionEngine"
    if [[ "$WITH_REMAINDER" == "true" ]]; then
        set_skipped "RemainderVerifier"
    fi
    save_deployment
    print_rollback_info
    exit 1
fi

header "Step 4/5: Deploy ExecutionEngine"
# ExecutionEngine(admin, programRegistry, verifier, feeRecipient)
if deploy_contract "ExecutionEngine" "src/ExecutionEngine.sol:ExecutionEngine" \
    "$DEPLOYER_ADDR" "$PROGRAM_REGISTRY_ADDR" "$MOCK_VERIFIER_ADDR" "$FEE_RECIPIENT"; then
    EXECUTION_ENGINE_ADDR="$LAST_DEPLOY_ADDR"
    validate_contract "ExecutionEngine" "$EXECUTION_ENGINE_ADDR" "owner()(address)" || true
    verify_contract "ExecutionEngine" "$EXECUTION_ENGINE_ADDR" "src/ExecutionEngine.sol:ExecutionEngine" \
        "address,address,address,address" "$DEPLOYER_ADDR" "$PROGRAM_REGISTRY_ADDR" "$MOCK_VERIFIER_ADDR" "$FEE_RECIPIENT"
    save_deployment
else
    DEPLOY_FAILED=true
    if [[ "$WITH_REMAINDER" == "true" ]]; then
        set_skipped "RemainderVerifier"
    fi
    save_deployment
    print_rollback_info
    exit 1
fi

header "Step 5/5: Deploy RemainderVerifier (optional)"
if [[ "$WITH_REMAINDER" == "true" ]]; then
    if deploy_contract "RemainderVerifier" "src/remainder/RemainderVerifier.sol:RemainderVerifier" "$DEPLOYER_ADDR"; then
        REMAINDER_VERIFIER_DEPLOYED_ADDR="$LAST_DEPLOY_ADDR"
        validate_contract "RemainderVerifier" "$REMAINDER_VERIFIER_DEPLOYED_ADDR" "owner()(address)" || true
        verify_contract "RemainderVerifier" "$REMAINDER_VERIFIER_DEPLOYED_ADDR" \
            "src/remainder/RemainderVerifier.sol:RemainderVerifier" \
            "address" "$DEPLOYER_ADDR"
        save_deployment

        # Wire RemainderVerifier into TEEMLVerifier
        if [[ "$DRY_RUN" == "false" ]]; then
            info "Wiring RemainderVerifier into TEEMLVerifier..."
            if cast send "$TEE_VERIFIER_ADDR" "setRemainderVerifier(address)" "$REMAINDER_VERIFIER_DEPLOYED_ADDR" \
                --rpc-url "$RPC_URL" --private-key "$PRIVATE_KEY" 2>&1; then
                ok "  TEEMLVerifier.remainderVerifier set to $REMAINDER_VERIFIER_DEPLOYED_ADDR"
            else
                warn "  Failed to wire RemainderVerifier. You may need to call setRemainderVerifier() manually."
            fi
        else
            warn "  [DRY RUN] Would wire RemainderVerifier into TEEMLVerifier"
        fi
    else
        DEPLOY_FAILED=true
        save_deployment
        print_rollback_info
        exit 1
    fi
else
    info "Skipped (use --with-remainder to deploy)"
    set_skipped "RemainderVerifier (not requested)"
fi

# ---------------------------------------------------------------------------
# Final save
# ---------------------------------------------------------------------------
save_deployment

# ---------------------------------------------------------------------------
# Post-deploy validation summary
# ---------------------------------------------------------------------------
if [[ "$DRY_RUN" == "false" ]]; then
    header "Post-Deploy Validation"
    VALIDATION_PASS=0
    VALIDATION_FAIL=0

    idx=0
    while [[ $idx -lt $DEPLOY_COUNT ]]; do
        v_name=$(get_deployed_name $idx)
        v_addr=$(get_deployed_addr $idx)

        # Skip pre-existing or dry-run entries
        case "$v_addr" in
            *DRY_RUN*|*"n/a"*|*"pre-existing"*)
                idx=$((idx + 1))
                continue
                ;;
        esac

        # Check that the contract has code
        code_size=$(cast codesize "$v_addr" --rpc-url "$RPC_URL" 2>/dev/null) || code_size="0"
        if [[ "$code_size" -gt 0 ]] 2>/dev/null; then
            ok "  $v_name ($v_addr): code size = $code_size bytes"
            VALIDATION_PASS=$((VALIDATION_PASS + 1))
        else
            err "  $v_name ($v_addr): NO CODE FOUND"
            VALIDATION_FAIL=$((VALIDATION_FAIL + 1))
        fi
        idx=$((idx + 1))
    done

    echo ""
    if [[ "$VALIDATION_FAIL" -gt 0 ]]; then
        warn "Validation: $VALIDATION_PASS passed, $VALIDATION_FAIL failed"
    else
        ok "Validation: all $VALIDATION_PASS contracts verified on-chain"
    fi
fi

# ---------------------------------------------------------------------------
# Summary table
# ---------------------------------------------------------------------------
header "============================================================"
header "  DEPLOYMENT SUMMARY"
header "============================================================"
echo ""
printf "  ${BOLD}%-25s %-44s %s${RESET}\n" "CONTRACT" "ADDRESS" "TX HASH"
printf "  %-25s %-44s %s\n" "-------------------------" "--------------------------------------------" "----------------------------------------------------------------"

idx=0
while [[ $idx -lt $DEPLOY_COUNT ]]; do
    s_name=$(get_deployed_name $idx)
    s_addr=$(get_deployed_addr $idx)
    s_tx=$(get_deployed_tx $idx)

    # Truncate tx hash for display if too long
    display_tx="$s_tx"
    if [[ ${#display_tx} -gt 66 ]]; then
        display_tx="${display_tx:0:66}"
    fi

    printf "  ${GREEN}%-25s${RESET} %-44s %s\n" "$s_name" "$s_addr" "$display_tx"
    idx=$((idx + 1))
done

if [[ $SKIP_COUNT -gt 0 ]]; then
    idx=0
    while [[ $idx -lt $SKIP_COUNT ]]; do
        sk_name=$(get_skipped_name $idx)
        printf "  ${YELLOW}%-25s${RESET} %-44s %s\n" "$sk_name" "--" "skipped"
        idx=$((idx + 1))
    done
fi

echo ""
printf "  ${BOLD}Chain:${RESET}      %s (id: %s)\n" "$CHAIN" "$CHAIN_ID"
printf "  ${BOLD}Deployer:${RESET}   %s\n" "$DEPLOYER_ADDR"
printf "  ${BOLD}Output:${RESET}     %s\n" "$DEPLOY_FILE"

if [[ "$DRY_RUN" == "true" ]]; then
    printf "  %sMode:%s       %sDRY RUN (no transactions broadcast)%s\n" "$BOLD" "$RESET" "$YELLOW" "$RESET"
fi

echo ""

# ---------------------------------------------------------------------------
# Next steps
# ---------------------------------------------------------------------------
if [[ "$DEPLOY_FAILED" == "false" && "$DRY_RUN" == "false" ]]; then
    header "Next Steps"
    echo "  1. Register programs:"
    echo "     cast send $PROGRAM_REGISTRY_ADDR 'registerProgram(bytes32,string,string,bytes32)' \\"
    echo "       <imageId> <name> <url> <schema> --rpc-url $RPC_URL --private-key \$DEPLOYER_PRIVATE_KEY"
    echo ""
    echo "  2. Register TEE enclaves:"
    echo "     cast send $TEE_VERIFIER_ADDR 'registerEnclave(address,bytes32)' \\"
    echo "       <enclaveKey> <imageHash> --rpc-url $RPC_URL --private-key \$DEPLOYER_PRIVATE_KEY"
    echo ""
    echo "  3. Submit execution requests:"
    echo "     cast send $EXECUTION_ENGINE_ADDR 'requestExecution(bytes32,bytes32,string,address,uint256,uint8)' \\"
    echo "       <imageId> <inputDigest> <inputUrl> 0x0 3600 0 --value 0.01ether \\"
    echo "       --rpc-url $RPC_URL --private-key \$DEPLOYER_PRIVATE_KEY"
    echo ""
fi

# ---------------------------------------------------------------------------
# Exit status
# ---------------------------------------------------------------------------
if [[ "$DEPLOY_FAILED" == "true" ]]; then
    exit 1
fi

ok "Deployment complete."
