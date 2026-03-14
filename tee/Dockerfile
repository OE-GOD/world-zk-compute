# Keep in sync with RUST_VERSION in .github/workflows/rust-ci.yml
FROM rust:1.92-slim AS builder
WORKDIR /app
COPY enclave/ .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
RUN adduser --disabled-password --gecos "" appuser
COPY --from=builder /app/target/release/tee-enclave /usr/local/bin/
COPY test-model/ /app/model/
RUN chown -R appuser:appuser /app
USER appuser
ENV MODEL_PATH=/app/model/model.json
ENV PORT=8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -sf http://localhost:8080/health || exit 1
CMD ["tee-enclave"]
