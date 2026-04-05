#!/bin/bash
# =============================================================================
# watch-wasm-build.sh -- Live watch monitor for wasm build
#
# Watch file changes in verifier crate directory and trigger wasm-pack.
# =============================================================================
set -euo pipefail

WASM_DIR="crates/zkml-verifier"
WEB_DIR="web"
RELOAD_STAMP="$WEB_DIR/.reload-stamp"

build() {
  echo "[wasm] Building..."
  wasm-pack build --target web "$WASM_DIR" -- --features wasm
  rm -rf "$WEB_DIR/pkg"
  cp -r "$WASM_DIR/pkg" "$WEB_DIR/pkg"
  date +%s > "$RELOAD_STAMP"
  echo "[wasm] Done."
}

if ! command -v fswatch >/dev/null 2>&1; then
  echo "[wasm] Error: fswatch is required but not installed."
  exit 1
fi

# Initial build
build

# Watch for source and manifest changes
fswatch -o "$WASM_DIR/src" "$WASM_DIR/Cargo.toml" | while read -r _; do
  build
done
