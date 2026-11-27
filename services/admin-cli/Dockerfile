# =============================================================================
# Admin CLI -- Multi-stage Docker Build
#
# Containerizes the admin-cli tool for contract management operations.
#
# Build context must be the project root (for sdk/ + crates/ + services/)
#
# Usage:
#   docker build -f services/admin-cli/Dockerfile -t worldzk-admin-cli .
#   docker run --rm worldzk-admin-cli --help
#   docker run --rm worldzk-admin-cli status --rpc-url http://host:8545 --contract 0x...
# =============================================================================

# ===== Stage 1: Build admin-cli =====
FROM rust:1.92-slim-bookworm AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends pkg-config libssl-dev git && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace root for dependency resolution
COPY Cargo.toml Cargo.lock ./

# Copy workspace dependencies
COPY sdk/ sdk/
COPY crates/ crates/
COPY services/admin-cli/ services/admin-cli/
# Stub out workspace members that aren't needed (cargo refuses to load the
# workspace unless every member listed in the root Cargo.toml has a manifest)
RUN mkdir -p cli/src && echo "fn main(){}" > cli/src/main.rs && \
    printf '[package]\nname = "worldzk-cli"\nversion = "0.1.0"\nedition = "2021"' > cli/Cargo.toml && \
    mkdir -p services/operator/src && echo "" > services/operator/src/lib.rs && echo "fn main(){}" > services/operator/src/main.rs && \
    printf '[package]\nname = "tee-operator"\nversion = "0.1.0"\nedition = "2021"\n[[bin]]\nname = "tee-operator"\npath = "src/main.rs"\n[lib]\npath = "src/lib.rs"' > services/operator/Cargo.toml && \
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
    --manifest-path services/admin-cli/Cargo.toml

# ===== Stage 2: Runtime =====
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

RUN adduser --disabled-password --gecos "" appuser

COPY --from=builder /build/target/release/admin-cli /usr/local/bin/admin-cli

USER appuser

ENTRYPOINT ["/usr/local/bin/admin-cli"]
CMD ["--help"]
