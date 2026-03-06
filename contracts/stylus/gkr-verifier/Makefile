# Stylus GKR Verifier — build, optimise, and measure WASM binary.
#
# Targets:
#   make build   — compile release WASM
#   make opt     — run wasm-opt (requires binaryen)
#   make size    — print raw + Brotli sizes
#   make check   — run cargo stylus check
#   make clean   — remove WASM artefacts

WASM_DIR  := target/wasm32-unknown-unknown/release
WASM_RAW  := $(WASM_DIR)/gkr_verifier.wasm
WASM_OPT  := $(WASM_DIR)/gkr_verifier_opt.wasm

.PHONY: build opt size check clean

build:
	cargo build --release --lib

opt: build
	wasm-opt $(WASM_RAW) -o $(WASM_OPT) --enable-bulk-memory -O3 --zero-filled-memory
	@echo "Optimised: $(WASM_OPT)"

size: opt
	@echo "=== WASM sizes ==="
	@RAW=$$(wc -c < $(WASM_RAW) | tr -d ' '); \
	 OPT=$$(wc -c < $(WASM_OPT) | tr -d ' '); \
	 BROTLI_RAW=$$(brotli --best --stdout $(WASM_RAW) | wc -c | tr -d ' '); \
	 BROTLI_OPT=$$(brotli --best --stdout $(WASM_OPT) | wc -c | tr -d ' '); \
	 echo "Raw:        $$RAW bytes ($$(echo "scale=1; $$RAW / 1024" | bc) KB)"; \
	 echo "Optimised:  $$OPT bytes ($$(echo "scale=1; $$OPT / 1024" | bc) KB)"; \
	 echo "Raw Brotli: $$BROTLI_RAW bytes ($$(echo "scale=1; $$BROTLI_RAW / 1024" | bc) KB)"; \
	 echo "Opt Brotli: $$BROTLI_OPT bytes ($$(echo "scale=1; $$BROTLI_OPT / 1024" | bc) KB)"

check:
	cargo stylus check

clean:
	rm -f $(WASM_OPT)
