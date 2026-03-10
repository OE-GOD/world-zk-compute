# ---- Stage 1: Build --------------------------------------------------------
FROM rust:1.80-bookworm AS builder

WORKDIR /build

# Copy manifest + lockfile first for layer caching
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src/ src/

# Build in release mode (only the main binary)
RUN cargo build --release --bin xgboost-remainder

# ---- Stage 2: Runtime ------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/xgboost-remainder /usr/local/bin/xgboost-remainder
COPY sample_model.json /app/model.json

EXPOSE 3000

ENTRYPOINT ["xgboost-remainder"]
CMD ["--model", "/app/model.json", "--serve", "--host", "0.0.0.0", "--port", "3000"]
