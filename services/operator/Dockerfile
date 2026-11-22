# ═══════════════════════════════════════════════════════════════════════════════
# TEE Operator Service — Multi-stage Docker Build
#
# Stage 1: Build precompute_proof (xgboost-remainder)
# Stage 2: Build tee-operator (services/operator)
# Stage 3: Minimal runtime image
#
# Build context must be the project root (for sdk/ + examples/ + services/)
# ═══════════════════════════════════════════════════════════════════════════════

# ===== Stage 1: Build precompute_proof =====
# Keep in sync with RUST_VERSION in .github/workflows/rust-ci.yml
FROM rust:1.92-slim AS prover-builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends pkg-config libssl-dev git && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY examples/xgboost-remainder/ examples/xgboost-remainder/
RUN cargo build --release --bin precompute_proof \
    --manifest-path examples/xgboost-remainder/Cargo.toml

# ===== Stage 2: Build operator =====
FROM rust:1.92-slim AS operator-builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends pkg-config libssl-dev git && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build
# Copy workspace root for dependency resolution
COPY Cargo.toml Cargo.lock ./
# Copy SDK and shared crates (operator depends on them via path)
COPY sdk/ sdk/
COPY crates/ crates/
COPY services/operator/ services/operator/
# Stub out workspace members that aren't needed (cargo refuses to load the
# workspace unless every member listed in the root Cargo.toml has a manifest)
RUN mkdir -p cli/src && echo "fn main(){}" > cli/src/main.rs && \
    printf '[package]\nname = "worldzk-cli"\nversion = "0.1.0"\nedition = "2021"' > cli/Cargo.toml && \
    mkdir -p services/admin-cli/src && echo "fn main(){}" > services/admin-cli/src/main.rs && \
    printf '[package]\nname = "admin-cli"\nversion = "0.1.0"\nedition = "2021"' > services/admin-cli/Cargo.toml && \
    mkdir -p services/indexer/src && echo "fn main(){}" > services/indexer/src/main.rs && \
    printf '[package]\nname = "tee-indexer"\nversion = "0.1.0"\nedition = "2021"' > services/indexer/Cargo.toml && \
    mkdir -p services/health-aggregator/src && echo "fn main(){}" > services/health-aggregator/src/main.rs && \
    printf '[package]\nname = "health-aggregator"\nversion = "0.1.0"\nedition = "2021"' > services/health-aggregator/Cargo.toml && \
    mkdir -p services/proof-registry/src && echo "fn main(){}" > services/proof-registry/src/main.rs && \
    printf '[package]\nname = "proof-registry"\nversion = "0.1.0"\nedition = "2021"' > services/proof-registry/Cargo.toml && \
    mkdir -p services/proof-generator/src && echo "fn main(){}" > services/proof-generator/src/main.rs && \
    printf '[package]\nname = "proof-generator"\nversion = "0.1.0"\nedition = "2021"' > services/proof-generator/Cargo.toml && \
    mkdir -p services/gateway/src && echo "fn main(){}" > services/gateway/src/main.rs && \
    printf '[package]\nname = "api-gateway"\nversion = "0.1.0"\nedition = "2021"' > services/gateway/Cargo.toml && \
    mkdir -p services/verifier/src && echo "fn main(){}" > services/verifier/src/main.rs && \
    printf '[package]\nname = "zkml-verifier-service"\nversion = "0.1.0"\nedition = "2021"' > services/verifier/Cargo.toml && \
    mkdir -p tee/enclave/src && echo "" > tee/enclave/src/lib.rs && echo "fn main(){}" > tee/enclave/src/main.rs && \
    printf '[package]\nname = "tee-enclave"\nversion = "0.1.0"\nedition = "2021"\n[lib]\nname = "tee_enclave"\npath = "src/lib.rs"\n[[bin]]\nname = "tee-enclave"\npath = "src/main.rs"' > tee/enclave/Cargo.toml && \
    mkdir -p tests/chaos/src && echo "" > tests/chaos/src/lib.rs && \
    printf '[package]\nname = "chaos-tests"\nversion = "0.1.0"\nedition = "2021"' > tests/chaos/Cargo.toml
RUN cargo build --release \
    --manifest-path services/operator/Cargo.toml

# ===== Stage 3: Runtime =====
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl jq && \
    rm -rf /var/lib/apt/lists/*

RUN adduser --disabled-password --gecos "" appuser

# Copy built binaries
COPY --from=prover-builder /build/examples/xgboost-remainder/target/release/precompute_proof /usr/local/bin/
COPY --from=operator-builder /build/target/release/tee-operator /usr/local/bin/

# Copy test model
COPY tee/test-model/ /app/model/

ENV MODEL_PATH=/app/model/model.json
ENV PROOFS_DIR=/app/proofs
ENV PRECOMPUTE_BIN=/usr/local/bin/precompute_proof

WORKDIR /app

# Default: read deployment addresses and start the operator
# The entrypoint script reads /deployment/addresses.json for contract addresses
COPY services/operator/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh && \
    mkdir -p /app/proofs && \
    chown -R appuser:appuser /app

USER appuser

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://localhost:9090/health || exit 1

ENTRYPOINT ["/app/entrypoint.sh"]
CMD ["watch"]
