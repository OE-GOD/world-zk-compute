# World ZK Compute Prover - Docker Image
#
# Build: docker build -t world-zk-prover .
# Run:   docker run -e PRIVATE_KEY=0x... -e RPC_URL=https://... world-zk-prover
#
# For GPU acceleration:
#   NVIDIA: docker build --build-arg FEATURES=cuda -t world-zk-prover .
#   Apple:  Not supported in Docker (use native build)

# Build stage
# Keep in sync with RUST_VERSION in .github/workflows/rust-ci.yml
FROM rust:1.92-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set build arguments
ARG FEATURES=""

WORKDIR /app

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock* ./

# Create dummy sources for dependency compilation (one per [[bin]]/[[bench]]
# target declared in Cargo.toml, otherwise cargo rejects the manifest)
RUN mkdir -p src && echo "fn main() {}" > src/main.rs \
    && mkdir -p src/bin && echo "fn main() {}" > src/bin/prove_benchmark.rs \
    && echo "fn main() {}" > src/bin/test_recursive_wrapper.rs \
    && mkdir -p benches && echo "fn main() {}" > benches/prover_benchmarks.rs

# Build dependencies only (cached layer)
RUN cargo build --release && rm -rf src benches

# Copy actual source code
COPY src ./src
COPY benches ./benches
COPY tests ./tests

# Touch main.rs to force rebuild
RUN touch src/main.rs

# Build the actual binary
RUN if [ -n "$FEATURES" ]; then \
        cargo build --release --features "$FEATURES"; \
    else \
        cargo build --release; \
    fi

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies (curl needed for HEALTHCHECK)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -u 1000 prover

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/world-zk-prover /usr/local/bin/

# Create data directory
RUN mkdir -p /app/data && chown -R prover:prover /app

# Switch to non-root user
USER prover

# Environment variables (override at runtime)
ENV RUST_LOG=info
ENV DATA_DIR=/app/data
# `run` serves /health and /metrics on HEALTH_PORT (default 8081); pin it to
# the port that HEALTHCHECK/EXPOSE below and prover/docker-compose.yml use.
ENV HEALTH_PORT=9090

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:9090/health || exit 1

# Expose API port
EXPOSE 9090

# Default command
ENTRYPOINT ["world-zk-prover"]
CMD ["run"]
