#!/usr/bin/env bash
# =============================================================================
# Migrate Deployment -- Generate Migration Plan Between Contract Versions
#
# A PLAN-ONLY tool: reads two deployment JSON files, compares contract
# addresses, and PRINTS the cast send commands needed to migrate state
# from the old deployment to the new one.  It does NOT execute any
# transactions.
#
# Usage:
#   ./scripts/migrate-deployment.sh \
#     --old-deployment deployments/11155111.json \
#     --new-deployment deployments/11155111-v2.json \
#     --rpc-url https://eth-sepolia.g.alchemy.com/v2/KEY
#
#   ./scripts/migrate-deployment.sh --help
#
# Exit codes:
#   0 -- migration plan generated successfully
#   1 -- error (missing files, bad JSON, missing tools)
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

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
migrate-deployment.sh -- Compare two deployment files and print a migration plan.

This is a PLAN tool.  It prints cast send commands but does NOT execute them.
Review the output carefully, then copy-paste the commands you need.

USAGE:
  scripts/migrate-deployment.sh [OPTIONS]

REQUIRED:
  --old-deployment <file>   Path to old deployment JSON
  --new-deployment <file>   Path to new deployment JSON
  --rpc-url <url>           RPC URL for the target chain

OPTIONS:
  --help, -h                Show this help message

DEPLOYMENT JSON FORMAT:
  Both files must contain a "contracts" object.  Two address formats are
  supported:

    Flat:     { "contracts": { "ExecutionEngine": "0x...", ... } }
    Nested:   { "contracts": { "ExecutionEngine": { "address": "0x..." }, ... } }

  Recognized contract names:
    ExecutionEngine, ProgramRegistry, TEEMLVerifier, RiscZeroVerifier,
    RemainderVerifier, MockRiscZeroVerifier

EXAMPLES:
  # Generate migration plan for Sepolia upgrade
  ./scripts/migrate-deployment.sh \
    --old-deployment deployments/11155111.json \
    --new-deployment deployments/11155111-v2.json \
    --rpc-url https://eth-sepolia.g.alchemy.com/v2/KEY

  # Generate plan for local Anvil
  ./scripts/migrate-deployment.sh \
    --old-deployment deployments/31337.json \
    --new-deployment deployments/31337-v2.json \
    --rpc-url http://127.0.0.1:8545
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
OLD_DEPLOY=""
NEW_DEPLOY=""
RPC_URL=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --old-deployment)
            OLD_DEPLOY="$2"
            shift 2
            ;;
        --new-deployment)
            NEW_DEPLOY="$2"
            shift 2
            ;;
        --rpc-url)
            RPC_URL="$2"
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
# Validate prerequisites
# ---------------------------------------------------------------------------
if [[ -z "$OLD_DEPLOY" ]]; then
    err "--old-deployment is required"
    echo ""
    usage
    exit 1
fi

if [[ -z "$NEW_DEPLOY" ]]; then
    err "--new-deployment is required"
    echo ""
    usage
    exit 1
fi

if [[ -z "$RPC_URL" ]]; then
    err "--rpc-url is required"
    echo ""
    usage
    exit 1
fi

if [[ ! -f "$OLD_DEPLOY" ]]; then
    err "Old deployment file not found: $OLD_DEPLOY"
    exit 1
fi

if [[ ! -f "$NEW_DEPLOY" ]]; then
    err "New deployment file not found: $NEW_DEPLOY"
    exit 1
fi

if ! command -v jq &>/dev/null; then
    err "jq is required.  Install: brew install jq"
    exit 1
fi

# Validate JSON
if ! jq . "$OLD_DEPLOY" >/dev/null 2>&1; then
    err "Old deployment file is not valid JSON: $OLD_DEPLOY"
    exit 1
fi

if ! jq . "$NEW_DEPLOY" >/dev/null 2>&1; then
    err "New deployment file is not valid JSON: $NEW_DEPLOY"
    exit 1
fi

# ---------------------------------------------------------------------------
# Address extraction helper
# ---------------------------------------------------------------------------
# Handles both flat ("ContractName": "0x...") and nested
# ("ContractName": { "address": "0x..." }) formats.
get_addr() {
    local file="$1"
    local name="$2"
    local raw
    raw=$(jq -r --arg n "$name" '.contracts[$n] // empty' "$file" 2>/dev/null)

    if [[ -z "$raw" || "$raw" == "null" ]]; then
        echo ""
        return
    fi

    # If the value starts with { it is a nested object -- extract .address
    if [[ "$raw" == "{"* ]]; then
        jq -r --arg n "$name" '.contracts[$n].address // empty' "$file" 2>/dev/null
    else
        echo "$raw"
    fi
}

# ---------------------------------------------------------------------------
# Read all contract addresses
# ---------------------------------------------------------------------------
CONTRACT_NAMES="ExecutionEngine ProgramRegistry TEEMLVerifier RiscZeroVerifier RemainderVerifier MockRiscZeroVerifier"

# Old addresses
OLD_EXECUTION_ENGINE=$(get_addr "$OLD_DEPLOY" "ExecutionEngine")
OLD_PROGRAM_REGISTRY=$(get_addr "$OLD_DEPLOY" "ProgramRegistry")
OLD_TEE_VERIFIER=$(get_addr "$OLD_DEPLOY" "TEEMLVerifier")
OLD_RISCZERO_VERIFIER=$(get_addr "$OLD_DEPLOY" "RiscZeroVerifier")
OLD_REMAINDER_VERIFIER=$(get_addr "$OLD_DEPLOY" "RemainderVerifier")
OLD_MOCK_VERIFIER=$(get_addr "$OLD_DEPLOY" "MockRiscZeroVerifier")

# New addresses
NEW_EXECUTION_ENGINE=$(get_addr "$NEW_DEPLOY" "ExecutionEngine")
NEW_PROGRAM_REGISTRY=$(get_addr "$NEW_DEPLOY" "ProgramRegistry")
NEW_TEE_VERIFIER=$(get_addr "$NEW_DEPLOY" "TEEMLVerifier")
NEW_RISCZERO_VERIFIER=$(get_addr "$NEW_DEPLOY" "RiscZeroVerifier")
NEW_REMAINDER_VERIFIER=$(get_addr "$NEW_DEPLOY" "RemainderVerifier")
NEW_MOCK_VERIFIER=$(get_addr "$NEW_DEPLOY" "MockRiscZeroVerifier")

# ---------------------------------------------------------------------------
# Comparison helper
# ---------------------------------------------------------------------------
# Returns: "unchanged", "changed", "added", "removed", or "absent"
compare_addr() {
    local old="$1"
    local new="$2"

    if [[ -z "$old" && -z "$new" ]]; then
        echo "absent"
    elif [[ -z "$old" && -n "$new" ]]; then
        echo "added"
    elif [[ -n "$old" && -z "$new" ]]; then
        echo "removed"
    elif [[ "$(printf '%s' "$old" | tr '[:upper:]' '[:lower:]')" == "$(printf '%s' "$new" | tr '[:upper:]' '[:lower:]')" ]]; then
        echo "unchanged"
    else
        echo "changed"
    fi
}

# ---------------------------------------------------------------------------
# Banner
# ---------------------------------------------------------------------------
header "============================================================"
header "  World ZK Compute -- Deployment Migration Plan"
header "============================================================"
echo ""
info "Old deployment: $OLD_DEPLOY"
info "New deployment: $NEW_DEPLOY"
info "RPC URL:        $RPC_URL"
echo ""
warn "This is a PLAN tool.  No transactions will be executed."
warn "Review the commands below, then run them manually."

# ---------------------------------------------------------------------------
# Step 1: Address comparison table
# ---------------------------------------------------------------------------
header "Step 1: Contract Address Comparison"
echo ""
printf "  ${BOLD}%-25s %-44s %-44s %s${RESET}\n" "CONTRACT" "OLD ADDRESS" "NEW ADDRESS" "STATUS"
printf "  %-25s %-44s %-44s %s\n" \
    "-------------------------" \
    "--------------------------------------------" \
    "--------------------------------------------" \
    "---------"

CHANGED_COUNT=0
ADDED_COUNT=0
REMOVED_COUNT=0
UNCHANGED_COUNT=0

print_row() {
    local name="$1"
    local old="$2"
    local new="$3"
    local status
    status=$(compare_addr "$old" "$new")

    local display_old="${old:-"(none)"}"
    local display_new="${new:-"(none)"}"

    case "$status" in
        unchanged)
            printf "  ${GREEN}%-25s${RESET} %-44s %-44s %s\n" "$name" "$display_old" "$display_new" "UNCHANGED"
            UNCHANGED_COUNT=$((UNCHANGED_COUNT + 1))
            ;;
        changed)
            printf "  ${YELLOW}%-25s${RESET} %-44s %-44s %s\n" "$name" "$display_old" "$display_new" "CHANGED"
            CHANGED_COUNT=$((CHANGED_COUNT + 1))
            ;;
        added)
            printf "  ${CYAN}%-25s${RESET} %-44s %-44s %s\n" "$name" "$display_old" "$display_new" "ADDED"
            ADDED_COUNT=$((ADDED_COUNT + 1))
            ;;
        removed)
            printf "  ${RED}%-25s${RESET} %-44s %-44s %s\n" "$name" "$display_old" "$display_new" "REMOVED"
            REMOVED_COUNT=$((REMOVED_COUNT + 1))
            ;;
        absent)
            # Skip contracts not present in either file
            ;;
    esac
}

print_row "ExecutionEngine"       "$OLD_EXECUTION_ENGINE"   "$NEW_EXECUTION_ENGINE"
print_row "ProgramRegistry"       "$OLD_PROGRAM_REGISTRY"   "$NEW_PROGRAM_REGISTRY"
print_row "TEEMLVerifier"         "$OLD_TEE_VERIFIER"       "$NEW_TEE_VERIFIER"
print_row "RiscZeroVerifier"      "$OLD_RISCZERO_VERIFIER"  "$NEW_RISCZERO_VERIFIER"
print_row "RemainderVerifier"     "$OLD_REMAINDER_VERIFIER" "$NEW_REMAINDER_VERIFIER"
print_row "MockRiscZeroVerifier"  "$OLD_MOCK_VERIFIER"      "$NEW_MOCK_VERIFIER"

echo ""
info "Summary: $CHANGED_COUNT changed, $ADDED_COUNT added, $REMOVED_COUNT removed, $UNCHANGED_COUNT unchanged"

# Track whether any migration commands were printed
MIGRATION_STEPS=0

# ---------------------------------------------------------------------------
# Step 2: TEEMLVerifier migration
# ---------------------------------------------------------------------------
TEE_STATUS=$(compare_addr "$OLD_TEE_VERIFIER" "$NEW_TEE_VERIFIER")
if [[ "$TEE_STATUS" == "changed" || "$TEE_STATUS" == "added" ]]; then
    header "Step 2: TEEMLVerifier Migration"
    echo ""

    if [[ "$TEE_STATUS" == "changed" ]]; then
        info "TEEMLVerifier address changed.  The new contract needs:"
        info "  - Ownership configuration (already set during deployment)"
        info "  - Enclave registrations re-applied"
        info "  - RemainderVerifier reference updated"
        echo ""

        # 2a: Transfer ownership on old contract (if Ownable2Step)
        info "2a. Transfer ownership of OLD TEEMLVerifier to new admin (optional)."
        info "    If you want the new contract's admin to also control the old one:"
        echo ""
        printf "    ${BOLD}# Initiate ownership transfer on old TEEMLVerifier${RESET}\n"
        echo "    cast send $OLD_TEE_VERIFIER \\"
        echo "      'transferOwnership(address)' <NEW_ADMIN_ADDRESS> \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
        printf "    ${BOLD}# Accept ownership on old TEEMLVerifier (Ownable2Step)${RESET}\n"
        echo "    cast send $OLD_TEE_VERIFIER \\"
        echo "      'acceptOwnership()' \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$NEW_ADMIN_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi

    local_tee="${NEW_TEE_VERIFIER:-$OLD_TEE_VERIFIER}"

    # 2b: Re-register enclaves on new TEEMLVerifier
    info "2b. Re-register all enclaves on the new TEEMLVerifier."
    info "    Query EnclaveRegistered events from the old contract to find enclave keys."
    echo ""
    printf "    ${BOLD}# List registered enclaves from old contract events${RESET}\n"
    echo "    cast logs --from-block 0 --to-block latest \\"
    echo "      --address $OLD_TEE_VERIFIER \\"
    echo "      'EnclaveRegistered(address indexed enclaveKey, bytes32 enclaveImageHash)' \\"
    echo "      --rpc-url $RPC_URL"
    echo ""
    printf "    ${BOLD}# For each enclave, register on the new contract${RESET}\n"
    echo "    cast send $local_tee \\"
    echo "      'registerEnclave(address,bytes32)' \\"
    echo "      <ENCLAVE_KEY_ADDRESS> <ENCLAVE_IMAGE_HASH> \\"
    echo "      --rpc-url $RPC_URL \\"
    echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
    echo ""
    MIGRATION_STEPS=$((MIGRATION_STEPS + 1))

    # 2c: Set RemainderVerifier on new TEEMLVerifier
    REMAINDER_TO_SET="${NEW_REMAINDER_VERIFIER:-$OLD_REMAINDER_VERIFIER}"
    if [[ -n "$REMAINDER_TO_SET" ]]; then
        info "2c. Set RemainderVerifier address on the new TEEMLVerifier."
        echo ""
        printf "    ${BOLD}# Point new TEEMLVerifier at RemainderVerifier${RESET}\n"
        echo "    cast send $local_tee \\"
        echo "      'setRemainderVerifier(address)' $REMAINDER_TO_SET \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi

    # 2d: Revoke enclaves on old contract (security best practice)
    if [[ "$TEE_STATUS" == "changed" ]]; then
        info "2d. Revoke enclaves on the OLD TEEMLVerifier (security hardening)."
        echo ""
        printf "    ${BOLD}# For each enclave registered on old contract${RESET}\n"
        echo "    cast send $OLD_TEE_VERIFIER \\"
        echo "      'revokeEnclave(address)' <ENCLAVE_KEY_ADDRESS> \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""

        info "2e. Pause the old TEEMLVerifier to prevent further submissions."
        echo ""
        printf "    ${BOLD}# Pause old TEEMLVerifier${RESET}\n"
        echo "    cast send $OLD_TEE_VERIFIER \\"
        echo "      'pause()' \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi
else
    header "Step 2: TEEMLVerifier Migration"
    if [[ "$TEE_STATUS" == "unchanged" ]]; then
        ok "TEEMLVerifier address unchanged -- no migration needed."
    elif [[ "$TEE_STATUS" == "absent" ]]; then
        info "TEEMLVerifier not present in either deployment -- skipping."
    elif [[ "$TEE_STATUS" == "removed" ]]; then
        warn "TEEMLVerifier removed from new deployment."
        warn "Ensure no services depend on it, then pause and revoke enclaves on old contract."
    fi
fi

# ---------------------------------------------------------------------------
# Step 3: ExecutionEngine migration
# ---------------------------------------------------------------------------
EE_STATUS=$(compare_addr "$OLD_EXECUTION_ENGINE" "$NEW_EXECUTION_ENGINE")
header "Step 3: ExecutionEngine Migration"

if [[ "$EE_STATUS" == "changed" || "$EE_STATUS" == "added" ]]; then
    echo ""
    info "ExecutionEngine address changed.  The new contract was initialized"
    info "with its own ProgramRegistry and verifier during deployment."
    echo ""

    # 3a: Pause old ExecutionEngine
    if [[ "$EE_STATUS" == "changed" && -n "$OLD_EXECUTION_ENGINE" ]]; then
        info "3a. Pause the old ExecutionEngine to stop new requests."
        echo ""
        printf "    ${BOLD}# Pause old ExecutionEngine${RESET}\n"
        echo "    cast send $OLD_EXECUTION_ENGINE \\"
        echo "      'pause()' \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi

    # 3b: Check if verifier reference needs updating
    # ExecutionEngine uses immutable verifier, so if the verifier changed the
    # whole ExecutionEngine must be redeployed. Print a note.
    info "3b. ExecutionEngine.verifier and ExecutionEngine.registry are immutable."
    info "    If the verifier or registry changed, a new ExecutionEngine deployment"
    info "    is required (which you already have in the new deployment file)."
    echo ""

    # 3c: Update fee recipient if needed
    if [[ -n "$NEW_EXECUTION_ENGINE" ]]; then
        info "3c. Verify fee recipient on the new ExecutionEngine."
        echo ""
        printf "    ${BOLD}# Check current fee recipient${RESET}\n"
        echo "    cast call $NEW_EXECUTION_ENGINE \\"
        echo "      'feeRecipient()(address)' \\"
        echo "      --rpc-url $RPC_URL"
        echo ""
        printf "    ${BOLD}# Update fee recipient if needed${RESET}\n"
        echo "    cast send $NEW_EXECUTION_ENGINE \\"
        echo "      'setFeeRecipient(address)' <FEE_RECIPIENT_ADDRESS> \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi

    # 3d: Transfer ownership
    if [[ -n "$NEW_EXECUTION_ENGINE" ]]; then
        info "3d. Transfer ownership of old ExecutionEngine (optional, for decommission)."
        echo ""
        printf "    ${BOLD}# Initiate ownership transfer (Ownable2Step)${RESET}\n"
        echo "    cast send $OLD_EXECUTION_ENGINE \\"
        echo "      'transferOwnership(address)' <NEW_ADMIN_ADDRESS> \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi
elif [[ "$EE_STATUS" == "unchanged" ]]; then
    echo ""
    ok "ExecutionEngine address unchanged -- no migration needed."

    # Even if the engine itself is unchanged, it might need its verifier updated
    # But verifier is immutable, so there is nothing to do for unchanged EE.
    VERIFIER_STATUS=$(compare_addr "$OLD_RISCZERO_VERIFIER" "$NEW_RISCZERO_VERIFIER")
    MOCK_STATUS=$(compare_addr "$OLD_MOCK_VERIFIER" "$NEW_MOCK_VERIFIER")
    if [[ "$VERIFIER_STATUS" == "changed" || "$MOCK_STATUS" == "changed" ]]; then
        echo ""
        warn "However, the verifier address changed.  ExecutionEngine.verifier"
        warn "is immutable -- you must redeploy ExecutionEngine to use the new verifier."
    fi
else
    echo ""
    if [[ "$EE_STATUS" == "absent" ]]; then
        info "ExecutionEngine not present in either deployment -- skipping."
    elif [[ "$EE_STATUS" == "removed" ]]; then
        warn "ExecutionEngine removed from new deployment.  Pause the old one:"
        echo ""
        echo "    cast send $OLD_EXECUTION_ENGINE \\"
        echo "      'pause()' \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
    fi
fi

# ---------------------------------------------------------------------------
# Step 4: ProgramRegistry migration
# ---------------------------------------------------------------------------
PR_STATUS=$(compare_addr "$OLD_PROGRAM_REGISTRY" "$NEW_PROGRAM_REGISTRY")
header "Step 4: ProgramRegistry Migration"

if [[ "$PR_STATUS" == "changed" || "$PR_STATUS" == "added" ]]; then
    echo ""
    info "ProgramRegistry address changed.  Programs must be re-registered"
    info "on the new contract."
    echo ""

    # 4a: Query old programs
    if [[ -n "$OLD_PROGRAM_REGISTRY" ]]; then
        info "4a. List all programs from the old ProgramRegistry."
        echo ""
        printf "    ${BOLD}# Get total program count${RESET}\n"
        echo "    cast call $OLD_PROGRAM_REGISTRY \\"
        echo "      'getProgramCount()(uint256)' \\"
        echo "      --rpc-url $RPC_URL"
        echo ""
        printf "    ${BOLD}# Get program IDs (paginated, offset=0, limit=100)${RESET}\n"
        echo "    cast call $OLD_PROGRAM_REGISTRY \\"
        echo "      'getAllPrograms(uint256,uint256)(bytes32[])' 0 100 \\"
        echo "      --rpc-url $RPC_URL"
        echo ""
        printf "    ${BOLD}# Get details for a specific program${RESET}\n"
        echo "    cast call $OLD_PROGRAM_REGISTRY \\"
        echo "      'getProgram(bytes32)(bytes32,address,string,string,bytes32,uint256,bool,bool,address,string)' \\"
        echo "      <IMAGE_ID> \\"
        echo "      --rpc-url $RPC_URL"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi

    # 4b: Re-register programs
    info "4b. Re-register each program on the new ProgramRegistry."
    echo ""
    printf "    ${BOLD}# Register program (basic, risc0 default)${RESET}\n"
    echo "    cast send ${NEW_PROGRAM_REGISTRY} \\"
    echo "      'registerProgram(bytes32,string,string,bytes32)' \\"
    echo "      <IMAGE_ID> <NAME> <PROGRAM_URL> <INPUT_SCHEMA> \\"
    echo "      --rpc-url $RPC_URL \\"
    echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
    echo ""
    printf "    ${BOLD}# Register program with custom verifier (Remainder, eZKL, etc.)${RESET}\n"
    echo "    cast send ${NEW_PROGRAM_REGISTRY} \\"
    echo "      'registerProgramWithVerifier(bytes32,string,string,bytes32,address,string)' \\"
    echo "      <IMAGE_ID> <NAME> <PROGRAM_URL> <INPUT_SCHEMA> \\"
    echo "      <VERIFIER_CONTRACT> <PROOF_SYSTEM> \\"
    echo "      --rpc-url $RPC_URL \\"
    echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
    echo ""
    MIGRATION_STEPS=$((MIGRATION_STEPS + 1))

    # 4c: Verify programs on new registry
    info "4c. Verify programs on the new ProgramRegistry (admin-only trust signal)."
    echo ""
    printf "    ${BOLD}# Mark program as verified${RESET}\n"
    echo "    cast send ${NEW_PROGRAM_REGISTRY} \\"
    echo "      'verifyProgram(bytes32)' <IMAGE_ID> \\"
    echo "      --rpc-url $RPC_URL \\"
    echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
    echo ""
    MIGRATION_STEPS=$((MIGRATION_STEPS + 1))

    # 4d: Pause old registry
    if [[ -n "$OLD_PROGRAM_REGISTRY" ]]; then
        info "4d. Pause the old ProgramRegistry to prevent new registrations."
        echo ""
        printf "    ${BOLD}# Pause old ProgramRegistry${RESET}\n"
        echo "    cast send $OLD_PROGRAM_REGISTRY \\"
        echo "      'pause()' \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi
elif [[ "$PR_STATUS" == "unchanged" ]]; then
    echo ""
    ok "ProgramRegistry address unchanged -- no migration needed."
else
    echo ""
    if [[ "$PR_STATUS" == "absent" ]]; then
        info "ProgramRegistry not present in either deployment -- skipping."
    elif [[ "$PR_STATUS" == "removed" ]]; then
        warn "ProgramRegistry removed from new deployment."
    fi
fi

# ---------------------------------------------------------------------------
# Step 5: RemainderVerifier migration
# ---------------------------------------------------------------------------
RV_STATUS=$(compare_addr "$OLD_REMAINDER_VERIFIER" "$NEW_REMAINDER_VERIFIER")
header "Step 5: RemainderVerifier Migration"

if [[ "$RV_STATUS" == "changed" || "$RV_STATUS" == "added" ]]; then
    echo ""
    info "RemainderVerifier address changed."
    echo ""

    # 5a: Update TEEMLVerifier to point at new RemainderVerifier
    local_tee="${NEW_TEE_VERIFIER:-$OLD_TEE_VERIFIER}"
    if [[ -n "$local_tee" ]]; then
        info "5a. Update TEEMLVerifier to point at the new RemainderVerifier."
        echo ""
        printf "    ${BOLD}# Set new RemainderVerifier on TEEMLVerifier${RESET}\n"
        echo "    cast send $local_tee \\"
        echo "      'setRemainderVerifier(address)' $NEW_REMAINDER_VERIFIER \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$DEPLOYER_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi

    # 5b: Re-register DAG circuits on new RemainderVerifier
    info "5b. Re-register DAG circuits on the new RemainderVerifier."
    info "    Check the old deployment for dagCircuit.circuitHash."
    echo ""

    OLD_CIRCUIT_HASH=$(jq -r '.dagCircuit.circuitHash // .circuit_hash // empty' "$OLD_DEPLOY" 2>/dev/null)
    if [[ -n "$OLD_CIRCUIT_HASH" && "$OLD_CIRCUIT_HASH" != "null" ]]; then
        info "    Old circuit hash found: $OLD_CIRCUIT_HASH"
        echo ""
    fi

    printf "    ${BOLD}# Re-register circuit on new RemainderVerifier (see deploy scripts for full args)${RESET}\n"
    echo "    # Circuit registration requires the full DAG description calldata."
    echo "    # Use the deploy-sepolia.sh or DeployAll.s.sol script to register circuits."
    echo ""
    MIGRATION_STEPS=$((MIGRATION_STEPS + 1))

    # 5c: Update ProgramRegistry entries that reference old RemainderVerifier
    local_registry="${NEW_PROGRAM_REGISTRY:-$OLD_PROGRAM_REGISTRY}"
    if [[ -n "$local_registry" && -n "$OLD_REMAINDER_VERIFIER" ]]; then
        info "5c. Update program entries that reference the old RemainderVerifier."
        echo ""
        printf "    ${BOLD}# Update verifier contract for a program${RESET}\n"
        echo "    cast send $local_registry \\"
        echo "      'updateVerifier(bytes32,address)' \\"
        echo "      <IMAGE_ID> $NEW_REMAINDER_VERIFIER \\"
        echo "      --rpc-url $RPC_URL \\"
        echo "      --private-key \$PROGRAM_OWNER_PRIVATE_KEY"
        echo ""
        MIGRATION_STEPS=$((MIGRATION_STEPS + 1))
    fi
elif [[ "$RV_STATUS" == "unchanged" ]]; then
    echo ""
    ok "RemainderVerifier address unchanged -- no migration needed."
else
    echo ""
    if [[ "$RV_STATUS" == "absent" ]]; then
        info "RemainderVerifier not present in either deployment -- skipping."
    elif [[ "$RV_STATUS" == "removed" ]]; then
        warn "RemainderVerifier removed from new deployment."
    fi
fi

# ---------------------------------------------------------------------------
# Step 6: Service/Config updates
# ---------------------------------------------------------------------------
header "Step 6: Service and Configuration Updates"
echo ""
info "After executing the migration commands above, update these files:"
echo ""
echo "  1. Environment files (.env, .env.sepolia, .env.production):"
echo "     Update contract addresses for any changed contracts."
echo ""
echo "  2. SDK configurations (sdk/typescript, sdk/python, sdk/rust):"
echo "     Update default contract addresses."
echo ""
echo "  3. Operator service (services/operator/):"
echo "     Restart with updated contract addresses."
echo ""
echo "  4. Indexer service (services/indexer/):"
echo "     Update start block and contract addresses, re-index if needed."
echo ""

printf "    ${BOLD}# Generate .env from new deployment file${RESET}\n"
echo "    ./scripts/generate-env-from-deployment.sh \\"
echo "      --deployment $NEW_DEPLOY \\"
echo "      --rpc-url $RPC_URL"
echo ""

# ---------------------------------------------------------------------------
# Step 7: Warnings about non-migratable data
# ---------------------------------------------------------------------------
header "Step 7: Non-Migratable Data (Manual Action Required)"
echo ""
printf "  ${YELLOW}WARNING: The following data CANNOT be automatically migrated:${RESET}\n"
echo ""
echo "  ExecutionEngine:"
echo "    - Pending execution requests (Pending/Claimed status)"
echo "      Requesters must cancel and re-submit on the new contract."
echo "    - In-progress claims (provers have active claim windows)"
echo "      Wait for all active claims to expire or complete before migration."
echo "    - Prover statistics (completedCount, earnings)"
echo "      Historical prover data is lost on the new contract."
echo "    - Protocol fee configuration"
echo "      Re-set on new contract if changed from default (2.5%)."
echo "    - Reputation contract reference"
echo "      Re-set on new contract if applicable."
echo ""
echo "  TEEMLVerifier:"
echo "    - Pending ML results (not yet finalized)"
echo "      Must be finalized or expire on the OLD contract."
echo "    - Active disputes (challenged but unresolved)"
echo "      Must be resolved on the OLD contract before decommission."
echo "    - Challenge bond and prover stake configuration"
echo "      Re-set on new contract if changed from defaults."
echo ""
echo "  ProgramRegistry:"
echo "    - Program ownership (msg.sender during registration)"
echo "      Programs will be owned by whoever re-registers them."
echo "    - Verification status (admin trust signals)"
echo "      Must be re-applied by admin on the new contract."
echo ""

# ---------------------------------------------------------------------------
# Step 8: Recommended migration order
# ---------------------------------------------------------------------------
header "Step 8: Recommended Migration Order"
echo ""
echo "  1. Deploy new contracts (already done -- new deployment file exists)"
echo "  2. Pause old contracts (ExecutionEngine, ProgramRegistry, TEEMLVerifier)"
echo "  3. Wait for all in-flight requests/disputes to complete on old contracts"
echo "  4. Re-register programs on new ProgramRegistry"
echo "  5. Re-register enclaves on new TEEMLVerifier"
echo "  6. Set RemainderVerifier on new TEEMLVerifier"
echo "  7. Verify programs on new ProgramRegistry (admin trust)"
echo "  8. Update service configurations and .env files"
echo "  9. Restart operator/indexer services"
echo " 10. Revoke enclaves on old TEEMLVerifier"
echo " 11. Monitor new deployment for correct operation"
echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  MIGRATION PLAN SUMMARY"
header "============================================================"
echo ""
printf "  ${BOLD}Contracts changed:${RESET}  %d\n" "$CHANGED_COUNT"
printf "  ${BOLD}Contracts added:${RESET}    %d\n" "$ADDED_COUNT"
printf "  ${BOLD}Contracts removed:${RESET}  %d\n" "$REMOVED_COUNT"
printf "  ${BOLD}Contracts unchanged:${RESET} %d\n" "$UNCHANGED_COUNT"
printf "  ${BOLD}Migration steps:${RESET}    %d\n" "$MIGRATION_STEPS"
echo ""

if [[ "$MIGRATION_STEPS" -eq 0 ]]; then
    ok "No migration steps needed -- deployments are identical."
else
    warn "Review all commands above before executing."
    warn "This plan does NOT execute any transactions."
    info "Copy the cast send commands and run them with the appropriate private key."
fi

echo ""
ok "Migration plan generated."
