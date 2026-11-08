# syntax=docker/dockerfile:1
# Build with a pinned compiler version. Resolve and pin verified image digests
# as part of each production release (builder AND runtime).
FROM rust:1.99.1-bookworm AS builder

WORKDIR /build
ARG DB_FEATURES=postgres,mysql

# Keep .env, .git and target outside the build context.
COPY Cargo.toml Cargo.lock rust-toolchain.toml build.rs ./
COPY vendor ./vendor
COPY src ./src
COPY migrations ./migrations
COPY public ./public

# Cargo caches stay outside image layers; only executables and runtime
# directories cross the stage boundary.
RUN --mount=type=cache,id=rust-api-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=rust-api-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=rust-api-cargo-target,target=/build/target,sharing=locked \
    case "$DB_FEATURES" in \
      postgres|mysql|postgres,mysql|mysql,postgres) ;; \
      *) echo "unsupported DB_FEATURES: $DB_FEATURES" >&2; exit 1 ;; \
    esac && \
    cargo build --locked --release --no-default-features --features "$DB_FEATURES" \
      --bin api --bin worker --bin scheduler --bin migrate --bin relay --bin healthcheck && \
    install -d -m 0755 /out/bin && \
    install -d -m 0750 /out/storage /out/storage/app \
      /out/storage/app/public /out/storage/app/private /out/storage/logs && \
    for bin in api worker scheduler migrate relay healthcheck; do \
      install -m 0755 "/build/target/release/$bin" "/out/bin/$bin"; \
    done && \
    rm -f /build/public/storage && \
    ln -s ../storage/app/public /build/public/storage

# Debian 13 distroless C/C++ runtime provides glibc/libgcc and CA roots for
# dynamically linked Rust. No apt, curl, bash, package manager or shell in runtime.
# Trivy still scans all OS + library packages; no ignore-unfixed/ignorefile bypass.
FROM gcr.io/distroless/cc-debian13:nonroot AS runtime

LABEL org.opencontainers.image.source="https://github.com/casudinlix/rust-api-skeleton" \
      org.opencontainers.image.description="Minimal non-root Rust API Skeleton runtime"

WORKDIR /app
COPY --from=builder --chown=10001:10001 /build/public /app/public
COPY --from=builder --chown=10001:10001 /out/storage /app/storage
COPY --from=builder --chown=10001:10001 /out/bin/ /usr/local/bin/

USER 10001:10001
# The Compose development profile intentionally overrides APP_ENV and
# OBSERVABILITY_HOST. Production observability stays loopback-only by default.
ENV APP_ENV=production APP_HOST=0.0.0.0 APP_PORT=3000 OBSERVABILITY_HOST=127.0.0.1
EXPOSE 3000
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=4s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/healthcheck", "9100"]
CMD ["/usr/local/bin/api"]
