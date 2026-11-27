# ===============================================================================
# TEE Indexer Service — Multi-stage Docker Build
#
# Build context must be the project root (for crates/ + services/)
# ===============================================================================

# ===== Stage 1: Build indexer =====
# Keep in sync with RUST_VERSION in .github/workflows/rust-ci.yml
FROM rust:1.92-slim AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends pkg-config libssl-dev git && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build
# Copy workspace root for dependency resolution
COPY Cargo.toml Cargo.lock ./
# Copy shared crates (indexer depends on crates/watcher via path)
COPY crates/ crates/
COPY services/indexer/ services/indexer/
# Stub out workspace members that aren't needed (cargo refuses to load the
# workspace unless every member listed in the root Cargo.toml has a manifest)
RUN mkdir -p sdk/src && echo "" > sdk/src/lib.rs && \
    printf '[package]\nname = "world-zk-sdk"\nversion = "0.1.0"\nedition = "2021"\n[lib]\npath = "src/lib.rs"' > sdk/Cargo.toml && \
    mkdir -p cli/src && echo "fn main(){}" > cli/src/main.rs && \
    printf '[package]\nname = "worldzk-cli"\nversion = "0.1.0"\nedition = "2021"' > cli/Cargo.toml && \
    mkdir -p services/operator/src && echo "" > services/operator/src/lib.rs && echo "fn main(){}" > services/operator/src/main.rs && \
    printf '[package]\nname = "tee-operator"\nversion = "0.1.0"\nedition = "2021"\n[[bin]]\nname = "tee-operator"\npath = "src/main.rs"\n[lib]\npath = "src/lib.rs"' > services/operator/Cargo.toml && \
    mkdir -p services/admin-cli/src && echo "fn main(){}" > services/admin-cli/src/main.rs && \
    printf '[package]\nname = "admin-cli"\nversion = "0.1.0"\nedition = "2021"' > services/admin-cli/Cargo.toml && \
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
    --manifest-path services/indexer/Cargo.toml \
    --features postgres

# ===== Stage 2: Runtime =====
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl jq && \
    rm -rf /var/lib/apt/lists/*

RUN adduser --disabled-password --gecos "" appuser

COPY --from=builder /build/target/release/tee-indexer /usr/local/bin/

RUN mkdir -p /data && chown appuser:appuser /data

WORKDIR /app

ENV PORT=8081
ENV DB_PATH=/data/indexer.db

EXPOSE 8081

USER appuser

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8081/health || exit 1

CMD ["tee-indexer"]
