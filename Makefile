# World ZK Compute — Developer Makefile
#
# Quick reference:
#   make test            Run all tests
#   make test-contracts  Run Solidity tests only
#   make test-rust       Run all Rust tests
#   make fmt             Format all code
#   make lint            Check formatting + linting
#   make build           Build all Rust crates
#   make deploy-local    Deploy to local Anvil
#   make clean           Clean all build artifacts

.PHONY: test test-contracts test-rust test-python test-ts test-operator test-enclave test-sdk \
        test-admin-cli test-indexer test-watcher-crate test-events-crate \
        build build-contracts build-rust \
        fmt fmt-sol fmt-rust lint lint-sol lint-rust \
        deploy-local deploy-sepolia deploy-multichain \
        docker-up docker-down docker-gpu \
        bench clean verify snapshot help \
        smoke-test audit docs

# ── Test ─────────────────────────────────────────────────────────────────────

test: ## Run all test suites
	@bash scripts/test-all.sh

test-fast: ## Run all tests except slow suites (xgboost-remainder)
	@bash scripts/test-all.sh --fast

test-contracts: ## Run Solidity tests
	cd contracts && forge test -vv

test-rust: ## Run all Rust tests (sdk, operator, enclave, admin-cli, indexer, watcher, events)
	cargo test --manifest-path sdk/Cargo.toml
	cargo test --manifest-path services/operator/Cargo.toml
	cargo test --manifest-path tee/enclave/Cargo.toml
	cargo test --manifest-path services/admin-cli/Cargo.toml
	cargo test --manifest-path services/indexer/Cargo.toml
	cargo test --manifest-path crates/watcher/Cargo.toml
	cargo test --manifest-path crates/events/Cargo.toml

test-operator: ## Run operator service tests
	cargo test --manifest-path services/operator/Cargo.toml

test-enclave: ## Run enclave tests
	cargo test --manifest-path tee/enclave/Cargo.toml

test-sdk: ## Run Rust SDK tests
	cargo test --manifest-path sdk/Cargo.toml

test-admin-cli: ## Run admin CLI tests
	cargo test --manifest-path services/admin-cli/Cargo.toml

test-indexer: ## Run indexer tests
	cargo test --manifest-path services/indexer/Cargo.toml

test-watcher-crate: ## Run shared watcher crate tests
	cargo test --manifest-path crates/watcher/Cargo.toml

test-events-crate: ## Run shared events crate tests
	cargo test --manifest-path crates/events/Cargo.toml

test-python: ## Run Python SDK tests
	cd sdk/python && python3 -m pytest tests/ -v

test-ts: ## Run TypeScript SDK tests
	cd sdk/typescript && npx vitest run --reporter=verbose

test-xgboost: ## Run XGBoost remainder tests (slow)
	cargo test --manifest-path examples/xgboost-remainder/Cargo.toml

test-stylus: ## Run Stylus GKR verifier tests (native target)
	cd contracts/stylus/gkr-verifier && cargo test --target $$(rustc -vV | awk '/^host:/{print $$2}')

# ── Build ────────────────────────────────────────────────────────────────────

build: build-contracts build-rust ## Build everything

build-contracts: ## Compile Solidity contracts
	cd contracts && forge build

build-rust: ## Build all Rust crates
	cargo build --manifest-path sdk/Cargo.toml
	cargo build --manifest-path services/operator/Cargo.toml
	cargo build --manifest-path tee/enclave/Cargo.toml
	cargo build --manifest-path services/admin-cli/Cargo.toml
	cargo build --manifest-path services/indexer/Cargo.toml
	cargo build --manifest-path crates/watcher/Cargo.toml
	cargo build --manifest-path crates/events/Cargo.toml
	cargo build --manifest-path examples/xgboost-remainder/Cargo.toml

# ── Format ───────────────────────────────────────────────────────────────────

fmt: fmt-sol fmt-rust ## Format all code

fmt-sol: ## Format Solidity files
	cd contracts && forge fmt

fmt-rust: ## Format Rust files
	cargo fmt --manifest-path sdk/Cargo.toml
	cargo fmt --manifest-path services/operator/Cargo.toml
	cargo fmt --manifest-path tee/enclave/Cargo.toml
	cargo fmt --manifest-path services/admin-cli/Cargo.toml
	cargo fmt --manifest-path services/indexer/Cargo.toml
	cargo fmt --manifest-path crates/watcher/Cargo.toml
	cargo fmt --manifest-path crates/events/Cargo.toml
	cargo fmt --manifest-path examples/xgboost-remainder/Cargo.toml

# ── Lint ─────────────────────────────────────────────────────────────────────

lint: lint-sol lint-rust ## Run all linters

lint-sol: ## Check Solidity formatting
	cd contracts && forge fmt --check

lint-rust: ## Run Rust clippy on all crates
	cargo clippy --manifest-path sdk/Cargo.toml -- -D warnings
	cargo clippy --manifest-path services/operator/Cargo.toml -- -D warnings
	cargo clippy --manifest-path tee/enclave/Cargo.toml -- -D warnings
	cargo clippy --manifest-path services/admin-cli/Cargo.toml -- -D warnings
	cargo clippy --manifest-path services/indexer/Cargo.toml -- -D warnings
	cargo clippy --manifest-path crates/watcher/Cargo.toml -- -D warnings
	cargo clippy --manifest-path crates/events/Cargo.toml -- -D warnings

# ── Deploy ───────────────────────────────────────────────────────────────────

deploy-local: ## Deploy to local Anvil (start Anvil first)
	DEPLOYER_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
	ARBITRUM_SEPOLIA_RPC=http://127.0.0.1:8545 \
	bash scripts/deploy-sepolia.sh

deploy-sepolia: ## Deploy to Arbitrum Sepolia (set DEPLOYER_PRIVATE_KEY)
	bash scripts/deploy-sepolia.sh

deploy-multichain: ## Deploy to all configured chains (set DEPLOYER_PRIVATE_KEY)
	bash scripts/deploy-multichain.sh

# ── Docker ───────────────────────────────────────────────────────────────────

docker-up: ## Start Docker Compose stack
	docker compose up -d

docker-down: ## Stop Docker Compose stack
	docker compose down

docker-gpu: ## Start GPU-enabled warm prover
	docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d warm-prover-gpu

# ── Benchmarks ───────────────────────────────────────────────────────────────

bench: ## Run Rust benchmarks
	cargo bench --manifest-path examples/xgboost-remainder/Cargo.toml 2>/dev/null || \
	echo "No benchmarks configured yet. See T117."

# ── Gas ──────────────────────────────────────────────────────────────────────

snapshot: ## Generate Solidity gas snapshot
	cd contracts && forge snapshot --match-contract GasProfileTest

snapshot-check: ## Check for gas regressions
	cd contracts && forge snapshot --check --match-contract GasProfileTest

gas-report: ## Print gas report for all tests
	cd contracts && forge test --gas-report

# ── Verification ─────────────────────────────────────────────────────────────

verify: ## Run post-deployment health check
	@echo "Usage: make verify RPC_URL=<url> CONTRACT=<address>"
	@echo "Example: make verify RPC_URL=http://127.0.0.1:8545 CONTRACT=0x..."
	@if [ -n "$(RPC_URL)" ] && [ -n "$(CONTRACT)" ]; then \
		bash scripts/verify-deployment.sh "$(RPC_URL)" "$(CONTRACT)"; \
	fi

# ── Smoke Test ──────────────────────────────────────────────────────────────

smoke-test: ## Run cross-service E2E smoke test
	bash scripts/smoke-test.sh

# ── Audit ───────────────────────────────────────────────────────────────────

audit: ## Run security audit on all dependencies
	cargo audit --manifest-path sdk/Cargo.toml 2>/dev/null || true
	cargo audit --manifest-path services/operator/Cargo.toml 2>/dev/null || true
	cargo audit --manifest-path tee/enclave/Cargo.toml 2>/dev/null || true
	cd sdk/python && pip-audit 2>/dev/null || true
	cd sdk/typescript && npm audit 2>/dev/null || true

# ── Docs ────────────────────────────────────────────────────────────────────

docs: ## Generate API documentation for all SDKs
	cargo doc --manifest-path sdk/Cargo.toml --no-deps 2>/dev/null || true
	cd sdk/typescript && npx typedoc 2>/dev/null || echo "Run: npm i -D typedoc"
	cd sdk/python && python3 -m pdoc worldzk -o docs/ 2>/dev/null || echo "Run: pip install pdoc"

# ── Clean ────────────────────────────────────────────────────────────────────

clean: ## Clean all build artifacts
	cd contracts && forge clean
	cargo clean --manifest-path sdk/Cargo.toml 2>/dev/null || true
	cargo clean --manifest-path services/operator/Cargo.toml 2>/dev/null || true
	cargo clean --manifest-path tee/enclave/Cargo.toml 2>/dev/null || true
	cargo clean --manifest-path services/admin-cli/Cargo.toml 2>/dev/null || true
	cargo clean --manifest-path services/indexer/Cargo.toml 2>/dev/null || true
	cargo clean --manifest-path crates/watcher/Cargo.toml 2>/dev/null || true
	cargo clean --manifest-path crates/events/Cargo.toml 2>/dev/null || true
	cargo clean --manifest-path examples/xgboost-remainder/Cargo.toml 2>/dev/null || true

# ── Help ─────────────────────────────────────────────────────────────────────

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
