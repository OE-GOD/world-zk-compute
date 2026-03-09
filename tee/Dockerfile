FROM rust:1.82-slim AS builder
WORKDIR /app
COPY enclave/ .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/tee-enclave /usr/local/bin/
COPY test-model/ /app/model/
ENV MODEL_PATH=/app/model/model.json
ENV PORT=8080
EXPOSE 8080
CMD ["tee-enclave"]
