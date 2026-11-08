# Docker builder and container security review

## 2026-11-08 CVE remediation candidate

PR #2 image-scanning run 42494522395 reported **46 HIGH/CRITICAL OS/package findings** (45 HIGH, 1 CRITICAL), mostly from the Debian 12 runtime and installed utilities such as curl. The Rust filesystem dependency and secret scan succeeded. Treat severity counts as scanner findings, not proof of exploitability; review Debian/vendor advisories and actual runtime reachability.

This security branch replaces the Debian 12 slim runtime with **Debian 13 Distroless cc:nonroot**. It removes apt, curl and a shell from the final stage. Compilation still happens in the pinned Rust builder stage. The image includes six Rust executables: API, worker, scheduler, migrate, relay, and the new std-only HTTP readiness probe. Health checks and Compose now use the probe instead of curl/CMD-SHELL. The probe only connects to 127.0.0.1 and checks the status of /ready; no request payloads, secrets or network destinations are logged.

**Verification status:** source patch authored. Docker/buildx/Trivy are unavailable in the authoring runtime. The branch's CI must build and scan the image before remediation can be declared successful. Keep both filesystem and image Trivy gates on HIGH/CRITICAL, with `ignore-unfixed: false`; do not add broad suppressions or silently lower severity. If findings remain, triage each package/version/CVE against the current fixed release and document any exception with rationale and expiry.

**Compatibility and rollout:** Distroless has no shell/curl/apt. Do not attempt `docker exec ... sh`; use logs, metrics, traces, a debug sibling image in a restricted environment or a dedicated diagnostic process instead. Keep the same UID/GID 10001, writable storage paths, built executables and /app/public/storage link. Before shipping, run live startup /ready and /health checks with disposable databases and Redis, check HTTPS certificate trust/SMTP, verify SIGTERM drain, and test writable /app/storage under a read-only root. Roll back to the previous image digest if any of these regress. Do not deploy a scanner failing on an untriaged critical CVE.

**Supply-chain requirement:** resolve and pin *verified* multi-architecture OCI digests for `rust:1.99.1-bookworm` and `gcr.io/distroless/cc-debian13:nonroot` when preparing a production release; the tags here are intentionally not presented as immutable digests. Re-scan after digest updates.


Reviewed: 2026-11-02. Scope: `Dockerfile`, `.dockerignore`, `compose.yaml`, `Cargo.lock`, `Cargo.toml`, and CI. Source-level audit only; **Docker is not available in the audit environment**, so image build and runtime security tests have not passed yet.

## What was already correct

- Multi-stage release build, Rust binaries only in the Debian slim runtime.
- `cargo build --locked`; `Cargo.lock` tracked.
- `[profile.release]` includes `strip = true` and `lto = "thin"`.
- Image runs as UID/GID 10001, not root.
- Development Compose uses a read-only root, drops all capabilities, sets `no-new-privileges`, publishes API and data services to loopback, and persists application storage.

## Remediations in this branch

- BuildKit target, Cargo registry, and Git cache mounts; only the six final binaries are copied into a persistent build layer.
- `DB_FEATURES` restricted to the supported combinations.
- Runtime defaults to `APP_ENV=production` and loopback-only observability; only port 3000 is advertised. Development Compose explicitly overrides these settings.
- PID cap, bounded `/tmp`, log rotation, `init: true`, and a fixed internal API port for Compose development.
- Dedicated image and filesystem vulnerability/secret/configuration scanning workflow, failing on HIGH/CRITICAL findings.
- Run-as-nonroot and artifact exclusion assertions in CI.

## Remaining production deployment requirements

1. Pin *verified* digest values for the Rust builder and Debian runtime images. Do not guess a digest: use `docker buildx imagetools inspect rust:1.99.1-bookworm` and `docker buildx imagetools inspect gcr.io/distroless/cc-debian13:nonroot`, review architecture and security updates, then pin the selected index digests.
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
docker run --rm --entrypoint /usr/local/bin/healthcheck rust-api-skeleton:security --help
docker system df -v
```

`docker system df -v` displays shared layers and build cache; Docker's filesystem footprint is not just the size of the final image. `docker builder prune` removes unused cache and can make future builds expensive; inspect it first.

Run `docker compose --profile tools run --rm migrate` only after configuring a disposable database, and smoke-test startup, health/readiness, database and Redis connectivity, and SIGTERM using non-production services. A build or scanner PASS is **not proof of zero vulnerabilities**.
