# Rust API Skeleton Implementation Plan

> **For agentic workers:** Apply the implementation and verification workflow to
> each independent module. Shared interfaces are fixed in `docs/contracts.md`.

**Goal:** Deliver a runnable modular Rust API, durable worker, UTC scheduler, and
both SQL drivers in the user's repository.

**Architecture:** Thin handlers call injected services/repositories. Typed pools
and dialect-specific queries isolate SQL differences. Durable jobs share the
application database and claim with expiring fenced leases.

**Tech Stack:** Axum 0.8, Tokio 1, SQLx 0.8.6, validator 0.20, jsonwebtoken 10,
Argon2id, tower-http 0.6, tokio-cron-scheduler 0.15.

**Spec:** `docs/superpowers/specs/2026-11-01-rust-api-design.md`

## Global constraints

- Preserve every requested module/path and expose API/worker/scheduler binaries.
- Support PostgreSQL and MySQL together and as independent Cargo features.
- Use parameterized SQL, UTC timestamps, secrets-safe errors, and no unsafe code.
- API-only: all application responses are JSON, except bodyless OPTIONS/HEAD.

## Review focus

1. Concurrent duplicate registration must not create an orphan welcome job.
2. Expired worker claims must not acknowledge/reject a newer worker's lease.
3. JWT identity and database role must agree; deleted users lose access.
4. Bad configuration and disabled database features fail before serving traffic.
5. MySQL UUID, boolean and UTC DATETIME decoding must match PostgreSQL behavior.

## Tasks

### 1. Configuration and contracts

- [x] Define Cargo features, module interfaces and deployment settings.
- [x] Test invalid secrets, driver/URL mismatch, CORS and lease/timeouts.
- [x] Implement typed loading and validation; add centralized safe errors.

### 2. Persistence

- [x] Implement dual pools, UTC sessions, migrations and user repository variants.
- [x] Test CRUD, cursor bounds, conflict mapping and transactional welcome enqueue.
- [x] Compile each feature set without a build-time database dependency.

### 3. Identity and services

- [x] Test DTO normalization, JWT expiry/issuer/audience and password verification.
- [x] Implement Argon2id, auth/user services, ownership/admin authorization guards.
- [x] Ensure secrets are absent from serialization and validation errors.

### 4. Jobs and scheduler

- [x] Implement claim/ack/retry/failure leases and explicit welcome mail transport.
- [x] Test contention, stale lease fencing, retry exhaustion and schedule deduplication.
- [x] Implement cancellable worker and scheduler with graceful shutdown.

### 5. HTTP and deployment

- [x] Implement handlers, routes, CORS/tracing, health/readiness and JSON rejection.
- [x] Test auth failures, access control, validation, health and method fallbacks.
- [x] Add Docker, Compose, Makefile, README and endpoint examples.

### 6. Integration and delivery

- [x] Run fmt, clippy, unit/HTTP tests, driver matrix and live database tests.
- [x] Review independently, resolve material findings and record verified results.
- [x] Prepare reviewed source and verification evidence for publication to `main`.

**Delivery decision:** The user explicitly requested committing the full
implementation directly to `main`. This supersedes the earlier branch/PR plan.
