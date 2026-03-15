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
FROM rust:1.94-slim-bookworm AS builder

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
