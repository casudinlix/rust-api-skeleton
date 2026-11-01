# syntax=docker/dockerfile:1
FROM rust:1.99.1-bookworm AS builder
WORKDIR /build
ARG DB_FEATURES=postgres,mysql
COPY Cargo.toml Cargo.lock rust-toolchain.toml build.rs ./
COPY src ./src
COPY migrations ./migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --locked --release --no-default-features --features "$DB_FEATURES" \
    --bin api --bin worker --bin scheduler --bin migrate

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 app \
    && useradd --uid 10001 --gid app --no-create-home --shell /usr/sbin/nologin app
WORKDIR /app
COPY --from=builder /build/target/release/api /build/target/release/worker /build/target/release/scheduler /build/target/release/migrate /usr/local/bin/
USER 10001:10001
ENV APP_HOST=0.0.0.0 APP_PORT=3000
EXPOSE 3000
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl --fail --silent --max-time 2 "http://127.0.0.1:${APP_PORT}/health" > /dev/null || exit 1
CMD ["api"]
