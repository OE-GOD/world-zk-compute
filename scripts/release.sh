#!/usr/bin/env bash
# =============================================================================
# Release Preparation Script for World ZK Compute
#
# Bumps version strings across the project, updates CHANGELOG.md, and creates
# a git tag. Does NOT push -- that is left to the developer.
#
# Usage:
#   ./scripts/release.sh --version 1.2.0
#   ./scripts/release.sh --version 1.2.0 --dry-run
#   ./scripts/release.sh --help
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
# Default flags
# ---------------------------------------------------------------------------
VERSION=""
DRY_RUN=false

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
release.sh -- Release preparation script for World ZK Compute.

USAGE:
  scripts/release.sh --version <X.Y.Z> [OPTIONS]

REQUIRED:
  --version <X.Y.Z>   Semantic version for the release (e.g. 1.2.0)

OPTIONS:
  --dry-run            Print what would happen but do not modify any files
  -h, --help           Show this help message

WHAT IT DOES:
  1. Validates version format (X.Y.Z)
  2. Checks you are on the main branch with a clean working tree
  3. Updates version strings in:
     - Cargo.toml files (workspace members and standalone crates)
     - sdk/typescript/package.json
     - sdk/python/pyproject.toml
  4. Updates CHANGELOG.md (moves "## Unreleased" to "## [X.Y.Z] - YYYY-MM-DD")
  5. Creates a git commit and tag vX.Y.Z (does NOT push)
  6. Prints next steps

EXAMPLES:
  ./scripts/release.sh --version 1.0.0
  ./scripts/release.sh --version 2.1.3 --dry-run
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
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
# Validate inputs
# ---------------------------------------------------------------------------
if [[ -z "$VERSION" ]]; then
    err "--version is required."
    echo ""
    usage
    exit 1
fi

# Accept both vX.Y.Z and X.Y.Z formats; strip leading v for internal use
VERSION="${VERSION#v}"

# Validate semver format: X.Y.Z where X, Y, Z are non-negative integers
if ! printf '%s' "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    err "Invalid version format: '$VERSION'. Must match vX.Y.Z or X.Y.Z (e.g. v1.2.0)"
    exit 1
fi

TAG_NAME="v${VERSION}"
RELEASE_DATE="$(date +%Y-%m-%d)"

header "============================================================"
header "  World ZK Compute -- Release Preparation"
header "============================================================"
echo ""
info "Version:      $VERSION"
info "Tag:          $TAG_NAME"
info "Date:         $RELEASE_DATE"

if [[ "$DRY_RUN" == "true" ]]; then
    warn "DRY RUN mode -- no files will be modified"
fi

echo ""

# ---------------------------------------------------------------------------
# Preflight checks
# ---------------------------------------------------------------------------
header "Preflight Checks"

# Must be in a git repo
if ! git -C "$PROJECT_ROOT" rev-parse --is-inside-work-tree &>/dev/null; then
    err "Not inside a git repository."
    exit 1
fi
ok "Inside git repository"

# Must be on main branch
CURRENT_BRANCH="$(git -C "$PROJECT_ROOT" rev-parse --abbrev-ref HEAD)"
if [[ "$CURRENT_BRANCH" != "main" ]]; then
    err "Must be on 'main' branch. Currently on: '$CURRENT_BRANCH'"
    exit 1
fi
ok "On branch: main"

# Working tree must be clean (staged + unstaged)
if ! git -C "$PROJECT_ROOT" diff --quiet 2>/dev/null || \
   ! git -C "$PROJECT_ROOT" diff --cached --quiet 2>/dev/null; then
    err "Working tree has uncommitted changes. Commit or stash them first."
    git -C "$PROJECT_ROOT" status --short >&2
    exit 1
fi
ok "Working tree is clean"

# Tag must not already exist
if git -C "$PROJECT_ROOT" rev-parse "$TAG_NAME" &>/dev/null; then
    err "Tag '$TAG_NAME' already exists. Delete it first or choose a different version."
    exit 1
fi
ok "Tag '$TAG_NAME' does not exist yet"

echo ""

# ---------------------------------------------------------------------------
# Run make preflight
# ---------------------------------------------------------------------------
header "Running Preflight Checks..."

if [[ -f "$PROJECT_ROOT/Makefile" ]] && command -v make &>/dev/null; then
    if [[ "$DRY_RUN" == "false" ]]; then
        info "Running: make -C $PROJECT_ROOT preflight"
        if ! make -C "$PROJECT_ROOT" preflight; then
            err "Preflight checks failed. Fix issues before releasing."
            exit 1
        fi
        ok "Preflight checks passed"
    else
        warn "[DRY RUN] Would run: make -C $PROJECT_ROOT preflight"
    fi
else
    warn "make or Makefile not found. Skipping preflight checks."
    warn "Run tests manually before releasing."
fi

echo ""

# ---------------------------------------------------------------------------
# Collect and update version files
# ---------------------------------------------------------------------------
header "Updating version files..."

FILES_UPDATED=0

# --- Cargo.toml files ---
# Update workspace members and standalone crates; skip third-party vendored libs.
CARGO_FILES=(
    "sdk/Cargo.toml"
    "prover/Cargo.toml"
    "services/operator/Cargo.toml"
    "services/admin-cli/Cargo.toml"
    "services/indexer/Cargo.toml"
    "crates/watcher/Cargo.toml"
    "crates/events/Cargo.toml"
    "tee/enclave/Cargo.toml"
    "private-input-server/Cargo.toml"
    "contracts/stylus/gkr-verifier/Cargo.toml"
    "tests/chaos/Cargo.toml"
    "tests/e2e/Cargo.toml"
    "tests/e2e/guest/Cargo.toml"
    "examples/xgboost-remainder/Cargo.toml"
    "examples/rule-engine/Cargo.toml"
    "examples/rule-engine/host/Cargo.toml"
    "examples/rule-engine/methods/Cargo.toml"
    "examples/rule-engine/methods/guest/Cargo.toml"
    "examples/anomaly-detector/Cargo.toml"
    "examples/anomaly-detector/host/Cargo.toml"
    "examples/anomaly-detector/methods/guest/Cargo.toml"
    "examples/recursive-wrapper/Cargo.toml"
    "examples/recursive-wrapper/methods/Cargo.toml"
    "examples/recursive-wrapper/methods/guest/Cargo.toml"
    "examples/xgboost-inference/methods/Cargo.toml"
    "examples/xgboost-inference/methods/guest/Cargo.toml"
    "examples/signature-verified/methods/guest/Cargo.toml"
    "examples/sybil-detector/methods/guest/Cargo.toml"
    "examples/sdk-rust-quickstart/Cargo.toml"
    "examples/anomaly-detector-sp1/Cargo.toml"
    "examples/anomaly-detector-sp1/program/Cargo.toml"
    "examples/anomaly-detector-sp1/script/Cargo.toml"
    "examples/rule-engine-sp1/Cargo.toml"
    "examples/rule-engine-sp1/program/Cargo.toml"
    "examples/rule-engine-sp1/script/Cargo.toml"
)

for cargo_rel in "${CARGO_FILES[@]}"; do
    cargo_file="$PROJECT_ROOT/$cargo_rel"
    if [[ ! -f "$cargo_file" ]]; then
        continue
    fi
    # Only update files that actually have a version field
    if ! grep -qE '^version\s*=\s*"' "$cargo_file" 2>/dev/null; then
        continue
    fi

    if [[ "$DRY_RUN" == "false" ]]; then
        sed -i.bak -E "s/^(version[[:space:]]*=[[:space:]]*\").*(\")/\1${VERSION}\2/" "$cargo_file"
        rm -f "${cargo_file}.bak"
        info "Updated: $cargo_rel"
    else
        warn "[DRY RUN] Would update: $cargo_rel"
    fi
    FILES_UPDATED=$((FILES_UPDATED + 1))
done

# --- package.json (TypeScript SDK) ---
TS_PACKAGE="$PROJECT_ROOT/sdk/typescript/package.json"
if [[ -f "$TS_PACKAGE" ]]; then
    if [[ "$DRY_RUN" == "false" ]]; then
        sed -i.bak -E 's/("version"[[:space:]]*:[[:space:]]*")[^"]*(")/\1'"${VERSION}"'\2/' "$TS_PACKAGE"
        rm -f "${TS_PACKAGE}.bak"
        info "Updated: sdk/typescript/package.json"
    else
        warn "[DRY RUN] Would update: sdk/typescript/package.json"
    fi
    FILES_UPDATED=$((FILES_UPDATED + 1))
fi

# --- Root package.json (if present) ---
ROOT_PACKAGE="$PROJECT_ROOT/package.json"
if [[ -f "$ROOT_PACKAGE" ]]; then
    if [[ "$DRY_RUN" == "false" ]]; then
        sed -i.bak -E 's/("version"[[:space:]]*:[[:space:]]*")[^"]*(")/\1'"${VERSION}"'\2/' "$ROOT_PACKAGE"
        rm -f "${ROOT_PACKAGE}.bak"
        info "Updated: package.json"
    else
        warn "[DRY RUN] Would update: package.json"
    fi
    FILES_UPDATED=$((FILES_UPDATED + 1))
fi

# --- pyproject.toml (Python SDK) ---
PY_PROJECT="$PROJECT_ROOT/sdk/python/pyproject.toml"
if [[ -f "$PY_PROJECT" ]]; then
    if [[ "$DRY_RUN" == "false" ]]; then
        sed -i.bak -E "s/^(version[[:space:]]*=[[:space:]]*\").*(\")/\1${VERSION}\2/" "$PY_PROJECT"
        rm -f "${PY_PROJECT}.bak"
        info "Updated: sdk/python/pyproject.toml"
    else
        warn "[DRY RUN] Would update: sdk/python/pyproject.toml"
    fi
    FILES_UPDATED=$((FILES_UPDATED + 1))
fi

ok "Processed $FILES_UPDATED version file(s)"
echo ""

# ---------------------------------------------------------------------------
# Update CHANGELOG.md
# ---------------------------------------------------------------------------
CHANGELOG="$PROJECT_ROOT/CHANGELOG.md"

header "Updating CHANGELOG.md..."

if [[ -f "$CHANGELOG" ]]; then
    if grep -q '## \[Unreleased\]' "$CHANGELOG" 2>/dev/null; then
        if [[ "$DRY_RUN" == "false" ]]; then
            # Insert a new empty Unreleased section and rename the old one
            sed -i.bak "s/## \[Unreleased\]/## [Unreleased]\n\n## [$VERSION] - $RELEASE_DATE/" "$CHANGELOG"
            rm -f "${CHANGELOG}.bak"
            info "Moved [Unreleased] -> [$VERSION] - $RELEASE_DATE"
        else
            warn "[DRY RUN] Would move [Unreleased] -> [$VERSION] - $RELEASE_DATE"
        fi
        ok "CHANGELOG.md updated"
    else
        warn "No '## [Unreleased]' section found in CHANGELOG.md. Skipping."
    fi
else
    warn "No CHANGELOG.md found. Skipping changelog update."
fi

echo ""

# ---------------------------------------------------------------------------
# Create git commit and tag
# ---------------------------------------------------------------------------
header "Creating release commit and tag..."

if [[ "$DRY_RUN" == "false" ]]; then
    cd "$PROJECT_ROOT"

    # Stage all modified version files
    git add -A

    # Check if there are actually changes to commit
    if git diff --cached --quiet 2>/dev/null; then
        warn "No changes to commit. Version files may already be at $VERSION."
    else
        git commit -m "release: ${TAG_NAME}

Bump version to ${VERSION} across all Cargo.toml, package.json,
and pyproject.toml files. Update CHANGELOG.md."
        ok "Created release commit"
    fi

    # Create annotated tag
    git tag -a "$TAG_NAME" -m "Release $TAG_NAME"
    ok "Created tag: $TAG_NAME"
else
    warn "[DRY RUN] Would create commit: 'release: ${TAG_NAME}'"
    warn "[DRY RUN] Would create tag: $TAG_NAME"
fi

echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  RELEASE SUMMARY"
header "============================================================"
echo ""
printf "  ${BOLD}Version:${RESET}       %s\n" "$VERSION"
printf "  ${BOLD}Tag:${RESET}           %s\n" "$TAG_NAME"
printf "  ${BOLD}Date:${RESET}          %s\n" "$RELEASE_DATE"
printf "  ${BOLD}Files updated:${RESET} %s\n" "$FILES_UPDATED"

if [[ "$DRY_RUN" == "true" ]]; then
    printf "  ${BOLD}Mode:${RESET}          ${YELLOW}DRY RUN (nothing was modified)${RESET}\n"
fi

echo ""

# ---------------------------------------------------------------------------
# Next steps
# ---------------------------------------------------------------------------
header "Next Steps"
if [[ "$DRY_RUN" == "true" ]]; then
    echo "  This was a dry run. To perform the actual release:"
    echo ""
    echo "    ./scripts/release.sh --version $VERSION"
    echo ""
else
    echo "  The release commit and tag have been created locally."
    echo "  To publish the release, run:"
    echo ""
    echo "    git push && git push --tags"
    echo ""
    echo "  To undo this release (before pushing):"
    echo ""
    echo "    git tag -d $TAG_NAME && git reset --soft HEAD~1"
    echo ""
fi

ok "Release preparation complete."
