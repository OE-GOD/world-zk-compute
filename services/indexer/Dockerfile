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

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:8081/health || exit 1

CMD ["tee-indexer"]
