# Local patches to apalis-redis 1.0.0

## Upstream provenance

This directory contains the published MIT-licensed `apalis-redis` **1.0.0**
source. Its [LICENSE](LICENSE), normalized `Cargo.toml`, `Cargo.toml.orig`,
README and `.cargo_vcs_info.json` are retained unchanged.

- Published archive: <https://static.crates.io/crates/apalis-redis/apalis-redis-1.0.0.crate>
- Archive SHA-256: `ea67c8cb8a2c5c71ff545bbcb63002e0b83f297bb789552fc900e1b650f460c1`
- Upstream repository: <https://github.com/apalis-dev/apalis-redis>
- Published VCS commit: `4b1e1b43edcccbcd404613c8c42832ab1c3af815`

The archive checksum was verified against the downloaded crate and the original
registry entry in the application lockfile. Cargo uses this directory through
`[patch.crates-io]`; the application still pins the dependency to `=1.0.0`.
The published dependency's own `Cargo.lock` and Cargo's extraction marker are
omitted; the application `Cargo.lock` controls dependency resolution.

## Why the patches are necessary

1. **Bound due-job promotion.** Upstream `push_tasks` places immediate deliveries
   in the scheduled sorted set at the current timestamp. Upstream `fetch_next`
   loads every due identifier and passes all of them to Lua `unpack` before
   fetching the configured batch. A backlog of 10,240 deliveries exceeds the
   Redis Lua stack in the regression fixture. Add `LIMIT 0 batch_size` to
   scheduled promotion. Every subsequent fetch can promote another batch.
2. **Bound orphan recovery.** The registration/heartbeat recovery path scans
   every expired worker and loads each complete inflight set. Pass the configured
   batch size as a recovery budget, limit candidate workers, and recover at most
   that many total jobs per invocation. Selection, requeue and reservation
   removal remain in one Lua invocation. Keep partially recovered expired
   workers registered so a later invocation can recover their remaining jobs.
   Never select workers whose heartbeat is newer than the expiry cutoff.

The application bounds `batch_size` through `QUEUE_CONCURRENCY` (1..64). These
changes do not alter payload formats, keys, authentication, job identities,
application retries, or the SQL execution ledger. Orphan set order was already
unspecified. The SQL outbox independently republishes unfinished application
jobs, including after transport loss.

The patch is intentionally limited to the two concrete unbounded hot paths.
Terminal retention and current-process registration cleanup are implemented in
`src/jobs/redis.rs`, not in this dependency. Upstream administrative operations
such as `vacuum` are not used by the application.

## Verification and removal condition

Focused live Redis regressions are in `jobs::redis::tests`:

- `redis_large_due_backlog_is_processed_in_bounded_batches`: all 10,240 due
  deliveries execute; the unpatched fetch script fails before yielding work.
- `redis_orphan_recovery_is_bounded_resumable_and_preserves_live_workers`: each
  recovery invocation respects its budget, partially recovered workers remain
  discoverable, repeated passes recover every orphan, and live workers survive.
- The remaining transport tests cover duplicate UUIDv7 deliveries, poison
  envelopes, concurrency, graceful cancellation, bounded terminal cleanup and
  sanitized connection failures.

Run `TEST_REDIS_URL=... make test-redis-transport` against a disposable instance.
The tests never flush a Redis database or delete unrelated namespace keys.

Remove this patch only after an upstream release provides equivalent bounds
for both paths and passes these regressions plus the PostgreSQL/MySQL queue
integration suites. Update the exact dependency pin and application lockfile,
remove `[patch.crates-io]` and this directory, and retain equivalent regression
coverage using the upstream public API. Review the new release's ACK, retry,
worker-registration and metadata semantics before changing the pin.

## Exact upstream diff

Only the following three upstream source files are modified. The other runtime
source, Lua scripts and licensing/provenance files remain byte-for-byte copies.

```diff
--- a/lua/fetch_next.lua
+++ b/lua/fetch_next.lua
@@ -25 +25 @@
--- 1. Promote any due scheduled jobs
+-- 1. Promote one bounded batch of due scheduled jobs.
@@ -30 +30,4 @@
-    ARGV[1]
+    ARGV[1],
+    'LIMIT',
+    0,
+    tonumber(ARGV[2])
--- a/lua/reenqueue_orphaned.lua
+++ b/lua/reenqueue_orphaned.lua
@@ -6 +6,2 @@
--- ARGV[2]: (optional) restrict to this specific worker only
+-- ARGV[2]: restrict to this specific worker; empty string means all expired workers
+-- ARGV[3]: maximum recovered jobs and candidate workers per call
@@ -8,0 +10 @@
+local budget = tonumber(ARGV[3])
@@ -11 +13 @@
-if ARGV[2] then
+if ARGV[2] and ARGV[2] ~= '' then
@@ -20 +22 @@
-    workers = redis.call("zrangebyscore", KEYS[1], 0, ARGV[1])
+    workers = redis.call("zrangebyscore", KEYS[1], 0, ARGV[1], 'LIMIT', 0, budget)
@@ -29 +31 @@
-    local jobs = redis.call("smembers", inflight_set_key)
+    local jobs = redis.call("srandmember", inflight_set_key, budget - processed)
@@ -34 +36 @@
-        redis.call("del", inflight_set_key)
+        redis.call("srem", inflight_set_key, unpack(jobs))
@@ -38 +40,7 @@
-    redis.call("zrem", KEYS[1], worker)
+    -- Keep a partially recovered worker discoverable by the next bounded pass.
+    if redis.call("scard", inflight_set_key) == 0 then
+        redis.call("zrem", KEYS[1], worker)
+    end
+    if processed >= budget then
+        break
+    end
--- a/src/queries/reenqueue.rs
+++ b/src/queries/reenqueue.rs
@@ -32,0 +33 @@
+                .arg(config.batch_size)
@@ -45,0 +47,2 @@
+                .arg("")
+                .arg(config.batch_size)
```
