# Redis Cache Implementation Plan

**Goal:** Add an optional reusable Redis cache and explain Redis worker support.

**Architecture:** A typed JSON facade wraps a byte-oriented cache store, with
Redis and disabled backends. AppState exposes the facade; authentication and SQL
queue behavior remain governed by the existing repositories and transactions.

**Tech stack:** redis-rs 1.8, Tokio, rustls, serde, existing Axum/SQLx services.

**Spec:** `docs/superpowers/specs/2026-11-01-redis-cache-design.md`

## Review focus

Invalid configuration must not leak URLs/passwords; Redis failure must not bypass
authorization; payload and TTL limits must apply to real commands; cache namespaces
must remain separate; optional Redis must not prevent migration/SQL queue startup.

## Tasks

- [x] Test and implement `CacheConfig` parsing/validation in `src/config/cache.rs`
  and `src/config/mod.rs`; disabled is the backwards-compatible default.
- [x] Test and implement `src/cache/`: `Cache::from_config(&CacheConfig)`, JSON
  `get`, `put` with optional TTL override, `forget`, `ping`, and loader-based
  `remember`; expose a pluggable async byte store and sanitized CacheError.
- [x] Add `pub cache: Arc<Cache>` to AppState and export the module. Leave current
  services opt-in; provide a compiling service usage example in rustdoc/tests.
- [x] Add real Redis integration tests, Makefile target, CI service, environment
  settings, development Compose profile, and cache/worker documentation.
- [x] Run fmt, clippy, unit/HTTP tests, driver matrix, Redis integration, database
  regression checks and release build; review the diff and record exact results.
- [x] Prepare the reviewed change for direct publication to `main`, as authorized
  in this conversation. The remote commit and tree verification are reported in
  the delivery response.
