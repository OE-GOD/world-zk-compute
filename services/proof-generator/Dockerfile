FROM rust:1.82-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY services/proof-generator/Cargo.toml services/proof-generator/Cargo.toml

# Create dummy main.rs to cache dependency compilation
RUN mkdir -p services/proof-generator/src && \
    echo 'fn main() {}' > services/proof-generator/src/main.rs

# Build dependencies only (this layer is cached unless Cargo.toml changes)
RUN cargo build --release -p proof-generator 2>/dev/null || true

# Copy actual source code
COPY services/proof-generator/src services/proof-generator/src

# Touch main.rs to force rebuild of our code (not dependencies)
RUN touch services/proof-generator/src/main.rs

# Build the actual binary
RUN cargo build --release -p proof-generator

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

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
