# Docker builder and container security review

Reviewed: 2026-11-02. Scope: `Dockerfile`, `.dockerignore`, `compose.yaml`, `Cargo.lock`, `Cargo.toml`, and CI. Source-level audit only; **Docker is not available in the audit environment**, so image build and runtime security tests have not passed yet.

## What was already correct

- Multi-stage release build, Rust binaries only in the Debian slim runtime.
- `cargo build --locked`; `Cargo.lock` tracked.
- `[profile.release]` includes `strip = true` and `lto = "thin"`.
- Image runs as UID/GID 10001, not root.
- Development Compose uses a read-only root, drops all capabilities, sets `no-new-privileges`, publishes API and data services to loopback, and persists application storage.

## Remediations in this branch

- BuildKit target, Cargo registry, and Git cache mounts; only the five final binaries are copied into a persistent build layer.
- `DB_FEATURES` restricted to the supported combinations.
- Runtime defaults to `APP_ENV=production` and loopback-only observability; only port 3000 is advertised. Development Compose explicitly overrides these settings.
- PID cap, bounded `/tmp`, log rotation, `init: true`, and a fixed internal API port for Compose development.
- Dedicated image and filesystem vulnerability/secret/configuration scanning workflow, failing on HIGH/CRITICAL findings.
- Run-as-nonroot and artifact exclusion assertions in CI.

## Remaining production deployment requirements

1. Pin *verified* digest values for the Rust builder and Debian runtime images. Do not guess a digest: use `docker buildx imagetools inspect rust:1.99.1-bookworm` and `docker buildx imagetools inspect debian:bookworm-slim`, review architecture and security updates, then pin the selected index digests.
2. Review security scan output and remediate real HIGH/CRITICAL findings in both base packages and Cargo dependencies. Rebuild regularly with `docker build --pull`.
3. Use a **separate production Compose/Orchestrator config**. This repository's `compose.yaml` is for development. Keep `read_only`, `cap_drop: [ALL]`, `no-new-privileges`; additionally set CPU/memory/PID limits per workload, encrypted network access to external services, secret injection, backup and data persistence, monitoring access restrictions, and graceful shutdown.
4. Never expose observability ports 9100–9103 publicly. In Compose they bind inside the network by design; enforce the boundary using Docker networks and firewall policies, not `EXPOSE` alone.
5. Never use the unauthenticated development Redis, local DB passwords, or `APP_ENV=local` in production. Do not put production secrets in `ARG`, `COPY`, source control, or the Docker build context.
6. Deploy application and migration processes with separate database identities. Back up before any migration and verify the rollback/restore plan.
7. Protect the default `main` branch with required reviews and working security checks. Branch protection is not currently enabled.
8. Configure rate limits, proxy trust, TLS termination, log redaction, auth, and per-tenant authorization independently of this container audit.

## Validation commands (not yet executed in audit environment)

```sh
docker compose config --quiet
docker build --pull --progress=plain -t rust-api-skeleton:security .
docker image inspect rust-api-skeleton:security --format '{{.Config.User}} {{.Size}}'
docker history --no-trunc rust-api-skeleton:security
docker run --rm --entrypoint /bin/sh rust-api-skeleton:security -ec 'id; ls -l /usr/local/bin/{api,worker,scheduler,migrate,relay}'
docker system df -v
```

`docker system df -v` displays shared layers and build cache; Docker's filesystem footprint is not just the size of the final image. `docker builder prune` removes unused cache and can make future builds expensive; inspect it first.

Run `docker compose --profile tools run --rm migrate` only after configuring a disposable database, and smoke-test startup, health/readiness, database and Redis connectivity, and SIGTERM using non-production services. A build or scanner PASS is **not proof of zero vulnerabilities**.
