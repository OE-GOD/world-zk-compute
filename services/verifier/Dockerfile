FROM rust:1.85-slim AS builder

WORKDIR /app

# Copy workspace files needed for build
COPY Cargo.toml Cargo.lock ./
COPY crates/zkml-verifier/ crates/zkml-verifier/
COPY services/verifier/ services/verifier/
# Create bench stub (not needed for build but referenced in Cargo.toml)
RUN mkdir -p crates/zkml-verifier/benches && \
    echo "fn main(){}" > crates/zkml-verifier/benches/verify_bench.rs
# Stub out workspace members that aren't needed
RUN mkdir -p sdk/src && echo "" > sdk/src/lib.rs && \
    printf '[package]\nname = "world-zk-sdk"\nversion = "0.1.0"\nedition = "2021"\n[lib]\npath = "src/lib.rs"' > sdk/Cargo.toml && \
    mkdir -p crates/watcher/src && echo "" > crates/watcher/src/lib.rs && \
    printf '[package]\nname = "tee-watcher"\nversion = "0.1.0"\nedition = "2021"\n[lib]\npath = "src/lib.rs"' > crates/watcher/Cargo.toml && \
    mkdir -p crates/events/src && echo "" > crates/events/src/lib.rs && \
    printf '[package]\nname = "tee-events"\nversion = "0.1.0"\nedition = "2021"\n[lib]\npath = "src/lib.rs"' > crates/events/Cargo.toml && \
    mkdir -p services/operator/src && echo "" > services/operator/src/lib.rs && echo "fn main(){}" > services/operator/src/main.rs && \
    printf '[package]\nname = "tee-operator"\nversion = "0.1.0"\nedition = "2021"\n[[bin]]\nname = "tee-operator"\npath = "src/main.rs"\n[lib]\npath = "src/lib.rs"' > services/operator/Cargo.toml && \
    mkdir -p services/admin-cli/src && echo "fn main(){}" > services/admin-cli/src/main.rs && \
    printf '[package]\nname = "admin-cli"\nversion = "0.1.0"\nedition = "2021"' > services/admin-cli/Cargo.toml && \
    mkdir -p services/indexer/src && echo "fn main(){}" > services/indexer/src/main.rs && \
    printf '[package]\nname = "tee-indexer"\nversion = "0.1.0"\nedition = "2021"' > services/indexer/Cargo.toml && \
    mkdir -p tee/enclave/src && echo "" > tee/enclave/src/lib.rs && echo "fn main(){}" > tee/enclave/src/main.rs && \
    printf '[package]\nname = "tee-enclave"\nversion = "0.1.0"\nedition = "2021"\n[lib]\nname = "tee_enclave"\npath = "src/lib.rs"\n[[bin]]\nname = "tee-enclave"\npath = "src/main.rs"' > tee/enclave/Cargo.toml && \
    mkdir -p tests/chaos/src && echo "" > tests/chaos/src/lib.rs && \
    printf '[package]\nname = "chaos-tests"\nversion = "0.1.0"\nedition = "2021"' > tests/chaos/Cargo.toml && \
    mkdir -p cli/src && echo "fn main(){}" > cli/src/main.rs && \
    printf '[package]\nname = "worldzk-cli"\nversion = "0.1.0"\nedition = "2021"' > cli/Cargo.toml && \
    mkdir -p services/gateway/src && echo "fn main(){}" > services/gateway/src/main.rs && \
    printf '[package]\nname = "api-gateway"\nversion = "0.1.0"\nedition = "2021"' > services/gateway/Cargo.toml && \
    mkdir -p services/proof-generator/src && echo "fn main(){}" > services/proof-generator/src/main.rs && \
    printf '[package]\nname = "proof-generator"\nversion = "0.1.0"\nedition = "2021"' > services/proof-generator/Cargo.toml && \
    mkdir -p services/proof-registry/src && echo "fn main(){}" > services/proof-registry/src/main.rs && \
    printf '[package]\nname = "proof-registry"\nversion = "0.1.0"\nedition = "2021"' > services/proof-registry/Cargo.toml && \
    mkdir -p services/health-aggregator/src && echo "fn main(){}" > services/health-aggregator/src/main.rs && \
    printf '[package]\nname = "health-aggregator"\nversion = "0.1.0"\nedition = "2021"' > services/health-aggregator/Cargo.toml

RUN cargo build --release -p zkml-verifier-service

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/zkml-verifier-service /usr/local/bin/

RUN groupadd -r verifier && useradd -r -g verifier verifier
USER verifier

ENV PORT=3000
EXPOSE 3000
CMD ["zkml-verifier-service"]
