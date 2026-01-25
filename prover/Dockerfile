# World ZK Compute Prover - Docker Image
#
# Build: docker build -t world-zk-prover .
# Run:   docker run -e PRIVATE_KEY=0x... -e RPC_URL=https://... world-zk-prover
#
# For GPU acceleration:
#   NVIDIA: docker build --build-arg FEATURES=cuda -t world-zk-prover .
#   Apple:  Not supported in Docker (use native build)

# Build stage
FROM rust:1.75-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set build arguments
ARG FEATURES=""

WORKDIR /app

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock* ./

# Create dummy main.rs for dependency compilation
RUN mkdir -p src && echo "fn main() {}" > src/main.rs

# Build dependencies only (cached layer)
RUN cargo build --release && rm -rf src

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

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
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

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:9090/health || exit 1

# Expose API port
EXPOSE 9090

# Default command
ENTRYPOINT ["world-zk-prover"]
CMD ["run"]
