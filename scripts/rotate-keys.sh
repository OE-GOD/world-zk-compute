#!/usr/bin/env bash
# Rotate signing keys for the World ZK Compute proof registry and API services.
#
# Usage:
#   ./scripts/rotate-keys.sh [--registry] [--api-keys] [--all] [--dry-run]
#
# Options:
#   --registry    Generate a new secp256k1 ECDSA signing key for the proof-registry
#   --api-keys    Generate new API keys for gateway, verifier, and indexer
#   --all         Rotate everything (registry + api-keys)
#   --dry-run     Show what would happen without making changes
#   --help, -h    Show this help message
#
# Environment variables (optional):
#   SOPS_FILE     Path to SOPS-encrypted env file to update in place
#                 (default: no auto-update; values printed to stdout)
#
# Examples:
#   ./scripts/rotate-keys.sh --registry
#   ./scripts/rotate-keys.sh --api-keys
#   ./scripts/rotate-keys.sh --all --dry-run
#   SOPS_FILE=deploy/secrets.env.encrypted ./scripts/rotate-keys.sh --all

set -euo pipefail

# ── Helpers ──────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

usage() {
    head -22 "$0" | tail -21
    exit 0
}

# Generate a cryptographically random hex string of N bytes.
random_hex() {
    local bytes="${1:-32}"
    if command -v openssl &>/dev/null; then
        openssl rand -hex "$bytes"
    elif [ -r /dev/urandom ]; then
        dd if=/dev/urandom bs=1 count="$bytes" 2>/dev/null | xxd -p | tr -d '\n'
    else
        error "No random source available (need openssl or /dev/urandom)"
        exit 1
    fi
}

# Generate a random API key: base64url, 32 bytes = 43 chars.
random_api_key() {
    if command -v openssl &>/dev/null; then
        openssl rand -base64 32 | tr '+/' '-_' | tr -d '='
    elif [ -r /dev/urandom ]; then
        dd if=/dev/urandom bs=32 count=1 2>/dev/null | base64 | tr '+/' '-_' | tr -d '='
    else
        error "No random source available"
        exit 1
    fi
}

# Derive the secp256k1 public key from a private key hex string.
# Requires either `cast` (Foundry) or `openssl`.
derive_public_key() {
    local privkey_hex="$1"

    if command -v cast &>/dev/null; then
        # cast wallet address accepts a private key and returns the address
        local address
        address=$(cast wallet address --private-key "0x${privkey_hex}" 2>/dev/null || true)
        if [ -n "$address" ]; then
            echo "$address"
            return 0
        fi
    fi

    # Fallback: just note that we cannot derive it here
    echo "(install Foundry 'cast' to derive address)"
    return 0
}

# ── Parse Arguments ──────────────────────────────────────────────────────────

ROTATE_REGISTRY=false
ROTATE_API_KEYS=false
DRY_RUN=false

if [ $# -eq 0 ]; then
    usage
fi

while [ $# -gt 0 ]; do
    case "$1" in
        --registry)
            ROTATE_REGISTRY=true
            ;;
        --api-keys)
            ROTATE_API_KEYS=true
            ;;
        --all)
            ROTATE_REGISTRY=true
            ROTATE_API_KEYS=true
            ;;
        --dry-run)
            DRY_RUN=true
            ;;
        --help|-h)
            usage
            ;;
        *)
            error "Unknown option: $1"
            usage
            ;;
    esac
    shift
done

# ── Validation ───────────────────────────────────────────────────────────────

SOPS_FILE="${SOPS_FILE:-}"

if [ -n "$SOPS_FILE" ] && [ "$DRY_RUN" = false ]; then
    if ! command -v sops &>/dev/null; then
        error "'sops' is required to update encrypted env files. Install: brew install sops"
        exit 1
    fi
    if [ ! -f "$SOPS_FILE" ]; then
        error "SOPS_FILE does not exist: $SOPS_FILE"
        exit 1
    fi
fi

# ── Banner ───────────────────────────────────────────────────────────────────

echo "============================================="
echo "  World ZK Compute -- Key Rotation"
echo "============================================="
echo ""
if [ "$DRY_RUN" = true ]; then
    warn "DRY RUN -- no changes will be made"
    echo ""
fi

# Collect all generated values for summary
declare -a SUMMARY_LINES=()

# ── Registry Signing Key ────────────────────────────────────────────────────

if [ "$ROTATE_REGISTRY" = true ]; then
    info "Generating new registry signing key (secp256k1)..."

    NEW_SIGNING_KEY=$(random_hex 32)
    NEW_SIGNING_ADDRESS=$(derive_public_key "$NEW_SIGNING_KEY")

    echo ""
    echo "  REGISTRY_SIGNING_KEY=${NEW_SIGNING_KEY}"
    echo "  Derived address:     ${NEW_SIGNING_ADDRESS}"
    echo ""

    SUMMARY_LINES+=("REGISTRY_SIGNING_KEY=${NEW_SIGNING_KEY}")

    if [ -n "$SOPS_FILE" ] && [ "$DRY_RUN" = false ]; then
        info "Updating $SOPS_FILE with new REGISTRY_SIGNING_KEY..."
        # Use sops set to update a single key (requires sops >= 3.8)
        # Fallback: decrypt, sed, re-encrypt
        TMPFILE=$(mktemp)
        sops decrypt "$SOPS_FILE" > "$TMPFILE"
        if grep -q "^REGISTRY_SIGNING_KEY=" "$TMPFILE"; then
            sed -i.bak "s|^REGISTRY_SIGNING_KEY=.*|REGISTRY_SIGNING_KEY=${NEW_SIGNING_KEY}|" "$TMPFILE"
        else
            echo "REGISTRY_SIGNING_KEY=${NEW_SIGNING_KEY}" >> "$TMPFILE"
        fi
        sops encrypt "$TMPFILE" > "$SOPS_FILE"
        rm -f "$TMPFILE" "${TMPFILE}.bak"
        info "Updated $SOPS_FILE"
    fi

    warn "ACTION REQUIRED: Restart proof-registry service to pick up the new key."
    warn "  systemctl restart proof-registry"
    warn "  kubectl rollout restart deployment/proof-registry"
    echo ""
fi

# ── API Keys ─────────────────────────────────────────────────────────────────

if [ "$ROTATE_API_KEYS" = true ]; then
    info "Generating new API keys..."
    echo ""

    NEW_GATEWAY_KEY=$(random_api_key)
    NEW_VERIFIER_KEY=$(random_api_key)
    NEW_ADMIN_KEY=$(random_api_key)

    echo "  GATEWAY_API_KEYS=${NEW_GATEWAY_KEY}"
    echo "  VERIFIER_API_KEYS=${NEW_VERIFIER_KEY}"
    echo "  ADMIN_API_KEY=${NEW_ADMIN_KEY}"
    echo ""

    SUMMARY_LINES+=("GATEWAY_API_KEYS=${NEW_GATEWAY_KEY}")
    SUMMARY_LINES+=("VERIFIER_API_KEYS=${NEW_VERIFIER_KEY}")
    SUMMARY_LINES+=("ADMIN_API_KEY=${NEW_ADMIN_KEY}")

    if [ -n "$SOPS_FILE" ] && [ "$DRY_RUN" = false ]; then
        info "Updating $SOPS_FILE with new API keys..."
        TMPFILE=$(mktemp)
        sops decrypt "$SOPS_FILE" > "$TMPFILE"

        for VAR in GATEWAY_API_KEYS VERIFIER_API_KEYS ADMIN_API_KEY; do
            local_val=""
            case "$VAR" in
                GATEWAY_API_KEYS)  local_val="$NEW_GATEWAY_KEY" ;;
                VERIFIER_API_KEYS) local_val="$NEW_VERIFIER_KEY" ;;
                ADMIN_API_KEY)     local_val="$NEW_ADMIN_KEY" ;;
            esac
            if grep -q "^${VAR}=" "$TMPFILE"; then
                sed -i.bak "s|^${VAR}=.*|${VAR}=${local_val}|" "$TMPFILE"
            else
                echo "${VAR}=${local_val}" >> "$TMPFILE"
            fi
        done

        sops encrypt "$TMPFILE" > "$SOPS_FILE"
        rm -f "$TMPFILE" "${TMPFILE}.bak"
        info "Updated $SOPS_FILE"
    fi

    warn "ROLLING UPDATE PROCEDURE:"
    warn "  1. Add new keys alongside old keys (comma-separated) in your env file"
    warn "  2. Deploy services (both old and new keys accepted)"
    warn "  3. Update all API clients to use the new keys"
    warn "  4. Remove old keys from the env file"
    warn "  5. Redeploy services"
    echo ""
fi

# ── Summary ──────────────────────────────────────────────────────────────────

if [ ${#SUMMARY_LINES[@]} -gt 0 ]; then
    echo "============================================="
    echo "  Summary of Generated Secrets"
    echo "============================================="
    echo ""
    for line in "${SUMMARY_LINES[@]}"; do
        echo "  $line"
    done
    echo ""
    if [ -n "$SOPS_FILE" ] && [ "$DRY_RUN" = false ]; then
        info "Secrets written to: $SOPS_FILE (SOPS-encrypted)"
    else
        warn "Secrets printed above. Store them securely."
        warn "To auto-update an encrypted env file, set SOPS_FILE:"
        warn "  SOPS_FILE=deploy/secrets.env.encrypted ./scripts/rotate-keys.sh --all"
    fi
    echo ""
    warn "SECURITY: Clear your terminal scrollback after copying these values."
    warn "  macOS: Cmd+K    Linux: clear && printf '\\033[3J'"
fi
