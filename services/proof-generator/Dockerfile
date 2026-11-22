FROM rust:1.92-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY services/proof-generator/Cargo.toml services/proof-generator/Cargo.toml
# Stub out workspace members that aren't needed (cargo refuses to load the
# workspace unless every member listed in the root Cargo.toml has a manifest)
RUN mkdir -p sdk/src && echo "" > sdk/src/lib.rs && \
    printf '[package]\nname = "world-zk-sdk"\nversion = "0.1.0"\nedition = "2021"\n[lib]\npath = "src/lib.rs"' > sdk/Cargo.toml && \
    mkdir -p cli/src && echo "fn main(){}" > cli/src/main.rs && \
    printf '[package]\nname = "worldzk-cli"\nversion = "0.1.0"\nedition = "2021"' > cli/Cargo.toml && \
    mkdir -p crates/watcher/src && echo "" > crates/watcher/src/lib.rs && \
    printf '[package]\nname = "tee-watcher"\nversion = "0.1.0"\nedition = "2021"\n[lib]\npath = "src/lib.rs"' > crates/watcher/Cargo.toml && \
    mkdir -p crates/events/src && echo "" > crates/events/src/lib.rs && \
    printf '[package]\nname = "tee-events"\nversion = "0.1.0"\nedition = "2021"\n[lib]\npath = "src/lib.rs"' > crates/events/Cargo.toml && \
    mkdir -p crates/zkml-verifier/src && echo "" > crates/zkml-verifier/src/lib.rs && \
    printf '[package]\nname = "zkml-verifier"\nversion = "0.1.0"\nedition = "2021"\n[lib]\npath = "src/lib.rs"' > crates/zkml-verifier/Cargo.toml && \
    mkdir -p services/operator/src && echo "" > services/operator/src/lib.rs && echo "fn main(){}" > services/operator/src/main.rs && \
    printf '[package]\nname = "tee-operator"\nversion = "0.1.0"\nedition = "2021"\n[[bin]]\nname = "tee-operator"\npath = "src/main.rs"\n[lib]\npath = "src/lib.rs"' > services/operator/Cargo.toml && \
    mkdir -p services/admin-cli/src && echo "fn main(){}" > services/admin-cli/src/main.rs && \
    printf '[package]\nname = "admin-cli"\nversion = "0.1.0"\nedition = "2021"' > services/admin-cli/Cargo.toml && \
    mkdir -p services/indexer/src && echo "fn main(){}" > services/indexer/src/main.rs && \
    printf '[package]\nname = "tee-indexer"\nversion = "0.1.0"\nedition = "2021"' > services/indexer/Cargo.toml && \
    mkdir -p services/health-aggregator/src && echo "fn main(){}" > services/health-aggregator/src/main.rs && \
    printf '[package]\nname = "health-aggregator"\nversion = "0.1.0"\nedition = "2021"' > services/health-aggregator/Cargo.toml && \
    mkdir -p services/proof-registry/src && echo "fn main(){}" > services/proof-registry/src/main.rs && \
    printf '[package]\nname = "proof-registry"\nversion = "0.1.0"\nedition = "2021"' > services/proof-registry/Cargo.toml && \
    mkdir -p services/gateway/src && echo "fn main(){}" > services/gateway/src/main.rs && \
    printf '[package]\nname = "api-gateway"\nversion = "0.1.0"\nedition = "2021"' > services/gateway/Cargo.toml && \
    mkdir -p services/verifier/src && echo "fn main(){}" > services/verifier/src/main.rs && \
    printf '[package]\nname = "zkml-verifier-service"\nversion = "0.1.0"\nedition = "2021"' > services/verifier/Cargo.toml && \
    mkdir -p tee/enclave/src && echo "" > tee/enclave/src/lib.rs && echo "fn main(){}" > tee/enclave/src/main.rs && \
    printf '[package]\nname = "tee-enclave"\nversion = "0.1.0"\nedition = "2021"\n[lib]\nname = "tee_enclave"\npath = "src/lib.rs"\n[[bin]]\nname = "tee-enclave"\npath = "src/main.rs"' > tee/enclave/Cargo.toml && \
    mkdir -p tests/chaos/src && echo "" > tests/chaos/src/lib.rs && \
    printf '[package]\nname = "chaos-tests"\nversion = "0.1.0"\nedition = "2021"' > tests/chaos/Cargo.toml

# Create dummy main.rs to cache dependency compilation
RUN mkdir -p services/proof-generator/src && \
    echo 'fn main() {}' > services/proof-generator/src/main.rs

# Build dependencies only (this layer is cached unless Cargo.toml changes)
RUN cargo build --release -p proof-generator

# Copy actual source code
COPY services/proof-generator/src services/proof-generator/src

# Touch main.rs to force rebuild of our code (not dependencies)
RUN touch services/proof-generator/src/main.rs

# Build the actual binary
RUN cargo build --release -p proof-generator

# Runtime stage
FROM debian:bookworm-slim

# curl is needed by the container healthcheck (docker-compose.offchain.yml)
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd --create-home --shell /bin/bash appuser

WORKDIR /app

COPY --from=builder /app/target/release/proof-generator /app/proof-generator

# Create data directory for model storage
RUN mkdir -p /app/data/models && chown -R appuser:appuser /app

USER appuser

EXPOSE 3100

ENV RUST_LOG=info

ENTRYPOINT ["/app/proof-generator"]
CMD ["--mock", "--host", "0.0.0.0", "--port", "3100", "--model-dir", "/app/data/models"]
