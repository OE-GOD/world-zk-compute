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
# Copy SDK (operator depends on it via path)
COPY sdk/ sdk/
COPY services/operator/ services/operator/
RUN cargo build --release \
    --manifest-path services/operator/Cargo.toml

# ===== Stage 3: Runtime =====
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl jq && \
    rm -rf /var/lib/apt/lists/*

RUN adduser --disabled-password --gecos "" appuser

# Copy built binaries
COPY --from=prover-builder /build/target/release/precompute_proof /usr/local/bin/
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

ENTRYPOINT ["/app/entrypoint.sh"]
CMD ["run"]
