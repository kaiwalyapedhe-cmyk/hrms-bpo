# syntax=docker/dockerfile:1.7
# ---------------------------------------------------------------------------
# hrms-api + hrms-seed — cargo-chef multi-stage build.
# Build context: repository root.
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
# Dependency layer: only rebuilt when Cargo.toml / Cargo.lock change.
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p hrms-api -p hrms-seed

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 curl tzdata \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --create-home hrms
ENV TZ=Asia/Kolkata \
    RUST_LOG=info,hrms_api=debug,sqlx=warn \
    BIND_ADDR=0.0.0.0:8080
WORKDIR /app
COPY --from=builder /app/target/release/hrms-api /usr/local/bin/hrms-api
COPY --from=builder /app/target/release/hrms-seed /usr/local/bin/hrms-seed
COPY --chown=hrms:hrms config /app/config
COPY --chown=hrms:hrms csv-templates /app/csv-templates
USER hrms
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=4s --start-period=30s --retries=5 \
  CMD curl -fsS http://127.0.0.1:8080/health/ready || exit 1
ENTRYPOINT ["/usr/local/bin/hrms-api"]
