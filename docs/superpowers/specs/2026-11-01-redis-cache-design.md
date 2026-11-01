# Redis cache driver

## Intent and scope

Add an optional Redis cache that application services can use through an injected
facade. The user also asked whether workers can use Redis; document that capability
and the distinction between cache storage and queue storage. Preserve the current
SQL queue and atomic user/welcome-job transaction.

## Design decisions

- `CACHE_DRIVER=none|redis`, default `none`; enabling Redis requires `REDIS_URL`.
- `Cache` exposes JSON `get`, `put`, `forget`, `remember`, and `ping` operations,
  backed by an object-safe byte-oriented store. Add `AppState.cache`.
- Use redis-rs 1.8 with Tokio/rustls and its reconnecting lazy connection manager.
  Construction validates configuration without requiring a live Redis server.
- Explicit cache operations return sanitized errors. `remember` falls back to its
  loader after a cache failure and returns the loader error if the loader fails.
  Cache writes are best-effort in `remember`; concurrent misses may load twice.
- All writes have a positive TTL, at most 30 days. Default TTL is 300 seconds.
- `CACHE_PREFIX` defaults to `rust-api-skeleton:cache:` and is 1..128 ASCII bytes
  from letters, digits, colon, underscore, hyphen and dot. Key suffixes use the
  same alphabet and are 1..256 bytes. Services must include tenant IDs where needed.
- `CACHE_TIMEOUT_MS` defaults to 500 and accepts 10..10000; it bounds each backend
  operation, including connection acquisition. The loader and synchronous JSON
  conversion have the caller's deadline. `CACHE_MAX_VALUE_BYTES` defaults to 1048576 and accepts
  64..16777216. Bound reads on the Redis side as well as serialized writes.
- No global flush commands, secrets in logs, implicit authentication caching,
  automatic user/profile caching, or changes to database readiness semantics.
- `redis://` and certificate-verified `rediss://` are supported. Reject query or
  fragment options, including insecure TLS bypasses, and sanitize URL errors.

## Alternatives considered

Direct Redis calls inside services would couple business code to a vendor API.
A generic cache with a disabled backend provides a reusable skeleton boundary
and keeps Redis optional in local development. Automatically caching users would
also require a separate invalidation/consistency design, so services opt in.

Moving the queue to Redis is a separate behavior change. A future Redis Streams
consumer-group worker should receive committed jobs through a SQL outbox/relay
and retain idempotency, acknowledgements, retry and recovery. Cache eviction and
queue persistence should use separate Redis instances for production workloads.

## Verification

Test configuration failures before implementation, then the real cache behavior:
disabled mode, JSON round-trip, misses, TTL, overwrite, deletion, prefixes,
invalid/corrupt/oversized values, loader fallback, unavailable Redis, authentication
errors and connection recovery. Add a Redis integration target and CI service.
Run the existing quality/driver checks and a release build. Document unrun checks.
