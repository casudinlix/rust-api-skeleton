# syntax=docker/dockerfile:1
# Multi-platform Docker Official Images, pinned to reviewed index digests.
# Update digests via Dependabot and re-scan before deploying.
FROM rust:1.99.1-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b AS builder

WORKDIR /build
ARG DB_FEATURES=postgres,mysql

# Copy only material required for compilation. No .env, .git, or target directory.
COPY Cargo.toml Cargo.lock rust-toolchain.toml build.rs ./
COPY vendor ./vendor
COPY src ./src
COPY migrations ./migrations

# A target cache avoids persisting multi-GB Rust intermediate objects as image
# layers. Copy the five finished executables outside the cache mount.
RUN --mount=type=cache,id=rust-api-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=rust-api-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=rust-api-cargo-target,target=/build/target,sharing=locked \
    case "$DB_FEATURES" in \
      postgres|mysql|postgres,mysql|mysql,postgres) ;; \
      *) echo "unsupported DB_FEATURES: $DB_FEATURES" >&2; exit 1 ;; \
    esac && \
    cargo build --locked --release --no-default-features --features "$DB_FEATURES" \
      --bin api --bin worker --bin scheduler --bin migrate --bin relay && \
    mkdir -p /out && \
    for bin in api worker scheduler migrate relay; do \
      install -m 0755 "/build/target/release/$bin" "/out/$bin"; \
    done

FROM debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3 AS runtime

LABEL org.opencontainers.image.source="https://github.com/casudinlix/rust-api-skeleton" \
      org.opencontainers.image.description="Minimal non-root Rust API Skeleton runtime"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 app \
    && useradd --uid 10001 --gid app --no-create-home --shell /usr/sbin/nologin app \
    && install -d -m 0750 -o app -g app \
        /app/storage/app/public /app/storage/app/private /app/storage/logs

WORKDIR /app
COPY --chown=10001:10001 public ./public
COPY --from=builder --chown=10001:10001 /out/ /usr/local/bin/
RUN rm -f /app/public/storage \
    && ln -s ../storage/app/public /app/public/storage

USER 10001:10001
# Standalone image defaults fail closed for production-only policies.
# Development Compose intentionally overrides APP_ENV and OBSERVABILITY_HOST.
ENV APP_ENV=production APP_HOST=0.0.0.0 APP_PORT=3000 OBSERVABILITY_HOST=127.0.0.1
# Do not advertise internal observability ports: docker run -P must not publish them.
EXPOSE 3000
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "--output", "/dev/null", "--noproxy", "*", "--max-time", "2", "http://127.0.0.1:9100/ready"]
CMD ["api"]
