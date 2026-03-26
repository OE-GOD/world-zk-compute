FROM rust:1.82-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY services/gateway/Cargo.toml services/gateway/Cargo.toml

# Create dummy main.rs to cache dependency compilation
RUN mkdir -p services/gateway/src && \
    echo 'fn main() {}' > services/gateway/src/main.rs

# Build dependencies only (this layer is cached unless Cargo.toml changes)
RUN cargo build --release -p api-gateway 2>/dev/null || true

# Copy actual source code
COPY services/gateway/src services/gateway/src

# Touch main.rs to force rebuild of our code (not dependencies)
RUN touch services/gateway/src/main.rs

# Build the actual binary
RUN cargo build --release -p api-gateway

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd --create-home --shell /bin/bash appuser

WORKDIR /app

COPY --from=builder /app/target/release/api-gateway /app/api-gateway

RUN chown -R appuser:appuser /app

USER appuser

EXPOSE 8080

ENV RUST_LOG=info
ENV GATEWAY_PORT=8080
ENV VERIFIER_URL=http://verifier:3000
ENV REGISTRY_URL=http://registry:3001
ENV GENERATOR_URL=http://generator:3002

ENTRYPOINT ["/app/api-gateway"]
