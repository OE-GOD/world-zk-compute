#!/usr/bin/env bash
# =============================================================================
# upload-private-input.sh -- Upload data to private input server
#
# Sends a file to the private input server via multipart/form-data POST.
# Returns the input hash and upload status.
#
# Required args:
#   FILE_PATH    Path to the file to upload
#
# Optional env:
#   PRIVATE_INPUT_SERVER_URL   Server URL (default: http://localhost:8090)
#
# Usage:
#   ./scripts/upload-private-input.sh <file>
#   ./scripts/upload-private-input.sh --request-id 0xabc123 <file>
#   ./scripts/upload-private-input.sh --help
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Color helpers (respects NO_COLOR)
# ---------------------------------------------------------------------------
if [ -z "${NO_COLOR:-}" ] && [ -t 1 ]; then
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    RED='\033[0;31m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    GREEN=''
    YELLOW=''
    CYAN=''
    RED=''
    BOLD=''
    RESET=''
fi

info()  { printf "%b[INFO]%b  %s\n" "$CYAN"   "$RESET" "$*"; }
ok()    { printf "%b[OK]%b    %s\n" "$GREEN"  "$RESET" "$*"; }
warn()  { printf "%b[WARN]%b  %s\n" "$YELLOW" "$RESET" "$*"; }
err()   { printf "%b[ERROR]%b %s\n" "$RED"    "$RESET" "$*" >&2; }

# ---------------------------------------------------------------------------
# Help
# ---------------------------------------------------------------------------
show_help() {
    cat <<'EOF'
upload-private-input.sh -- Upload data to private input server

USAGE:
  ./scripts/upload-private-input.sh [OPTIONS] <FILE_PATH>

ARGUMENTS:
  FILE_PATH    Path to the file to upload (JSON, binary, etc.)

OPTIONS:
  --request-id <id>    Associate the upload with an execution request ID
  --server <url>       Override server URL (default: PRIVATE_INPUT_SERVER_URL or
                       http://localhost:8090)
  -h, --help           Show this help and exit

OPTIONAL ENVIRONMENT:
  PRIVATE_INPUT_SERVER_URL   Server URL (default: http://localhost:8090)

OUTPUT:
  Prints the server response including input hash and upload status.

EXAMPLES:
  ./scripts/upload-private-input.sh data/input.json
  ./scripts/upload-private-input.sh --request-id 0xabc123 data/input.json
  PRIVATE_INPUT_SERVER_URL=http://remote:8090 ./scripts/upload-private-input.sh data.bin
EOF
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
REQUEST_ID=""
SERVER_URL=""
FILE_PATH=""

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            show_help
            exit 0
            ;;
        --request-id)
            REQUEST_ID="$2"
            shift 2
            ;;
        --server)
            SERVER_URL="$2"
            shift 2
            ;;
        -*)
            err "Unknown option: $1"
            echo "Run with --help for usage."
            exit 1
            ;;
        *)
            if [ -z "$FILE_PATH" ]; then
                FILE_PATH="$1"
            else
                err "Unexpected argument: $1 (FILE_PATH already set to $FILE_PATH)"
                exit 1
            fi
            shift
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate
# ---------------------------------------------------------------------------
if ! command -v curl &>/dev/null; then
    err "'curl' not found. Please install curl."
    exit 1
fi

if [ -z "$FILE_PATH" ]; then
    err "FILE_PATH argument is required."
    echo "  Usage: ./scripts/upload-private-input.sh <file>"
    echo "  Run with --help for details."
    exit 1
fi

if [ ! -f "$FILE_PATH" ]; then
    err "File not found: $FILE_PATH"
    exit 1
fi

# Resolve server URL
BASE_URL="${SERVER_URL:-${PRIVATE_INPUT_SERVER_URL:-http://localhost:8090}}"

# Strip trailing slash
BASE_URL="${BASE_URL%/}"

# ---------------------------------------------------------------------------
# Print header
# ---------------------------------------------------------------------------
FILE_SIZE=$(wc -c < "$FILE_PATH" | tr -d ' ')
FILE_NAME=$(basename "$FILE_PATH")

printf "\n"
printf "%b========================================%b\n" "$BOLD" "$RESET"
printf "%b  Upload Private Input%b\n" "$BOLD" "$RESET"
printf "%b========================================%b\n" "$BOLD" "$RESET"
printf "\n"

info "Server:       $BASE_URL"
info "File:         $FILE_PATH"
info "File name:    $FILE_NAME"
info "File size:    $FILE_SIZE bytes"

if [ -n "$REQUEST_ID" ]; then
    info "Request ID:   $REQUEST_ID"
fi

printf "\n"

# ---------------------------------------------------------------------------
# Check server reachability
# ---------------------------------------------------------------------------
info "Checking server connectivity..."

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    --connect-timeout 5 \
    "${BASE_URL}/health" 2>/dev/null || echo "000")

if [ "$HTTP_CODE" = "000" ]; then
    err "Cannot reach server at $BASE_URL"
    echo "  Make sure the private input server is running."
    echo "  Default: http://localhost:8090"
    exit 1
fi

ok "Server reachable (HTTP $HTTP_CODE)"

# ---------------------------------------------------------------------------
# Build curl command
# ---------------------------------------------------------------------------
UPLOAD_URL="${BASE_URL}/upload"
CURL_ARGS=(-s -w "\n%{http_code}" --connect-timeout 10 --max-time 120)
CURL_ARGS+=(-X POST)
CURL_ARGS+=(-F "file=@${FILE_PATH}")

if [ -n "$REQUEST_ID" ]; then
    CURL_ARGS+=(-F "request_id=${REQUEST_ID}")
fi

# ---------------------------------------------------------------------------
# Upload
# ---------------------------------------------------------------------------
info "Uploading $FILE_NAME to $UPLOAD_URL ..."

RESPONSE=$(curl "${CURL_ARGS[@]}" "$UPLOAD_URL" 2>&1)

# Split response body and HTTP status code
RESP_CODE=$(echo "$RESPONSE" | tail -1)
RESP_BODY=$(echo "$RESPONSE" | sed '$d')

printf "\n"

if [ "$RESP_CODE" -ge 200 ] 2>/dev/null && [ "$RESP_CODE" -lt 300 ] 2>/dev/null; then
    ok "Upload successful (HTTP $RESP_CODE)"
    printf "\n"
    printf "%b--- Response ---%b\n" "$BOLD" "$RESET"
    echo "$RESP_BODY"
    printf "\n"

    # Try to extract input hash from JSON response
    INPUT_HASH=""
    if command -v jq &>/dev/null; then
        INPUT_HASH=$(echo "$RESP_BODY" | jq -r '.input_hash // .hash // .inputHash // empty' 2>/dev/null || echo "")
    elif command -v python3 &>/dev/null; then
        INPUT_HASH=$(echo "$RESP_BODY" | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    print(d.get('input_hash', d.get('hash', d.get('inputHash', ''))))
except Exception:
    print('')
" 2>/dev/null || echo "")
    fi

    if [ -n "$INPUT_HASH" ]; then
        ok "Input hash: $INPUT_HASH"
    fi
else
    err "Upload failed (HTTP $RESP_CODE)"
    if [ -n "$RESP_BODY" ]; then
        printf "\n"
        printf "%b--- Response ---%b\n" "$BOLD" "$RESET"
        echo "$RESP_BODY"
    fi
    exit 1
fi
