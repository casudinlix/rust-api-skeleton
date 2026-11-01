# Verification record

Date: **1 November 2026 (Asia/Jakarta)**.

The checks below were executed against this implementation in an isolated Linux
environment. They are verification evidence for the skeleton, not production
capacity measurements or a claim that a live deployment has been performed.

## Environment

| Component | Tested version |
|---|---|
| Rust | 1.99.1, x86_64-unknown-linux-gnu |
| SQLx | 0.8.6 |
| Axum | 0.8.9 |
| PostgreSQL | 16.15 |
| MySQL | 8.0.46, InnoDB |

Cargo.lock records the complete resolved dependency graph. Compose and GitHub
Actions target PostgreSQL 16 and MySQL 8.4. The local MySQL integration evidence
here is specifically for 8.0.46; it must not be described as a local 8.4 run.

## Executed checks

| Check | Result |
|---|---|
| All 46 paths explicitly requested in the specification | Present |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --locked --all-targets` | 49 unit tests + 9 HTTP contract tests passed |
| `cargo check --locked --all-targets --no-default-features --features postgres` | Passed |
| `cargo check --locked --all-targets --no-default-features --features mysql` | Passed |
| `cargo build --locked --release --bins` | Passed; optimized native binaries built |
| `make test-postgres` with disposable database | API, queue, repository suites all passed |
| `make test-mysql` with disposable database | API, queue, repository suites all passed |
| Five additional MySQL queue contention runs | All passed |
| Native process smoke test | Migration, API, two workers, two schedulers passed |
| SIGTERM smoke test | All five running service processes exited with code 0 |
| Compose and CI YAML parsing | Passed |

The normal all-target command intentionally ignores six integration tests.
Those six tests were then executed explicitly: three suites per database, each
containing multiple behavior assertions. Total executed named tests: **64**
(58 without a live database, plus six live-database suites).

## Behavior covered

- DTO normalization before validation, password boundaries, safe validation
  metadata, and unknown-field rejection including attempted role assignment.
- Argon2id salt uniqueness, correct/incorrect password verification, unknown-user
  work, bounded hashing concurrency, and retained permits after cancellation.
- JWT algorithm/signature/issuer/audience/expiry/nbf/iat/subject validation,
  current database role, deleted-user rejection, and strict Bearer parsing.
- JSON error contract, body limits, CORS, auth rate limits, route/method fallback,
  request IDs, health, and readiness failure when the database is unavailable.
- Readiness before migration, after migration, and with an unsuccessful migration;
  unit cases also cover missing/unknown/future versions and checksum mismatch.
- Idempotent migration application, UTC sessions, parameterized CRUD, normalized
  duplicate emails, concurrent duplicate registration, and transactional rollback
  if welcome-job insertion fails.
- Keyset pagination with tied timestamps, deleted cursor rows, invalid limits,
  invalid cursor format, and timestamps outside the shared database range.
- Concurrent queue consumers, exact eventual job set, unique owner tokens,
  unexpired-lease protection, forged/stale/expired ACK and failure transitions,
  retry availability, terminal attempts, final-attempt crash recovery,
  deduplication retention, and preservation of pending/failed jobs during pruning.
- Shared async shutdown deadline, bounded pool closure, cooperative drain, and
  cancellation of idle worker waits.

## Native process smoke test

The smoke test created a disposable PostgreSQL database and ran the real
`migrate` binary. It then started one API, two independent worker processes and
two independent scheduler processes, with local log email transport and a fast
test cron expression. It registered a user through HTTP, retrieved the profile
using the returned JWT, and observed:

```text
send_welcome_email:completed:1
maintenance:completed:1
```

All processes received SIGTERM and exited successfully. The database was removed
after the test. No real email was sent.

## Findings corrected during verification

1. **MySQL binary collation:** SQLx identified binary-collated text as a BINARY
   wire type. MySQL adapters now decode bytes explicitly and validate UTF-8/UUIDs.
   Public API errors remain sanitized.
2. **Readiness:** A bare SELECT 1 did not detect missing migrations. Readiness now
   validates the exact embedded migration history, success flags and checksums.
3. **Shutdown:** Pool closure originally happened after the drain timeout. It now
   uses the same absolute deadline, and scheduler enqueue waits observe cancellation.
4. **Queue contention test:** The original assertion incorrectly required every
   job to be reserved in the first concurrent wave. MySQL SKIP LOCKED can return
   a temporary empty result while another claim transaction locks scanned rows.
   The test now uses bounded synchronized polling and checks the exact eventual
   job set, unique IDs/tokens and first-attempt ownership. No isolation-level or
   lease correctness guarantee was weakened.

An independent static review checked the implementation and then reviewed the
readiness, shutdown and MySQL corrections. No outstanding blocking finding was
reported in that targeted re-review.

## Verification limits

- The Dockerfile was reviewed; a Docker image build/run was not executed in this
  environment. YAML parsing does not replace `docker compose config` or an image build.
  The pinned `rust:1.99.1-bookworm` tag was confirmed in the official image manifest.
- Actual SMTP delivery/provider authentication and certificate negotiation were
  not exercised. The process smoke test used the explicitly simulated log transport.
- No production infrastructure was deployed, no load-test RPS claim is made,
  and no production database was accessed.
- CI configuration is included; this record describes local execution, not a
  guarantee of GitHub runner availability, credit balance, or remote CI outcome.
- Async shutdown is bounded; already-running Tokio blocking password tasks may
  delay runtime exit. Orchestrator force-stop remains the ultimate process bound.

## Redis cache extension — 1 November 2026

The initial verification record above describes the original skeleton. The
following checks were executed after adding optional Redis caching; these are the
current test counts for the extended implementation.

Environment: redis-rs **1.8.1**, Redis server **8.0.5**, PostgreSQL **16.15**, MySQL
**8.0.46**, and the same Rust/Axum/SQLx versions recorded above. Redis ran as an
isolated loopback instance with test-only authentication and no persistence.

| Check | Result |
|---|---|
| `make check`: formatting and Clippy with warnings denied | Passed |
| Unit tests | 63 passed |
| HTTP contract tests | 9 passed |
| Rustdoc usage example | 1 passed (compilation) |
| PostgreSQL-only and MySQL-only compilation | Both passed |
| `make test-postgres` | API, queue and repository suites passed |
| `make test-mysql` | API, queue and repository suites passed |
| `make test-redis` | All 7 Redis integration tests passed |
| `cargo build --locked --release --bins` | Passed; optimized binaries rebuilt |
| Compose/CI YAML and relative documentation links | Passed |

The ordinary unit/HTTP command ignores 13 network integration tests; all 13 were
then executed explicitly. Total: **85 named unit/integration tests**, plus the
compiled rustdoc example. No ignored test is counted as executed.

### Redis behaviors verified

- Default disabled caching creates no connection and retains normal loader behavior.
- Redis construction requires neither a Tokio runtime nor a reachable server.
- Typed JSON round trips, misses, stored `null`, prefix separation, overwrite,
  mandatory default/override TTL, actual expiry, and single-key deletion.
- Invalid key/TTL/configuration rejection, bounded JSON serialization, atomic
  Redis-side read-size checks, malformed payload handling, and sanitized errors.
- `remember` serves hits, does not cache loader errors, and preserves successful
  loader results when Redis reads or writes fail.
- Deadlines cover backend read/write/delete/ping operations; connection refusal,
  silent peers and authentication failures remain bounded.
- A test-owned TCP proxy drops an established connection; the same Cache instance
  reconnects and retrieves its existing value. The Redis server is not restarted.
- Initial authentication failure recovers after a uniquely named test ACL user
  is created. That ACL user is removed by the test; global flush is never used.
- Existing API authorization, database readiness and transactional SQL queue
  regression suites pass with the additional cache wiring.

An independent review found that storing an unconnected lazy manager could retain
an initial authentication failure. A failing regression reproduced the problem.
The implementation now awaits the first successful connection inside the lazy
initializer before retaining the manager, and the regression passes. Final review
reported no remaining blocking finding.

### Scope and remaining verification limits

This change implements a cache driver. Redis queue transport, SQL outbox relay and
Redis-backed worker dispatch are not implemented; the existing DatabaseQueue is
unchanged. No user, role, password hash or JWT is automatically cached.

The Redis integration endpoint used `redis://`. `rediss://` support and rejection
of insecure options are configured/tested at the URL layer, but a real TLS
handshake was not exercised. Docker execution, production load and real SMTP
delivery remain unverified. The earlier native multi-process smoke test describes
the initial skeleton and was not repeated for this cache-only extension.

As documented in `docs/cache.md`, credential/ACL/SELECT handshake corrections
after an established manager's reconnect may require recreating Cache or restarting
the process with redis-rs 1.8.1. `remember` continues to use its loader during the
failure. This is distinct from the initial authentication and ordinary network
recovery scenarios exercised above. CI now includes Redis; remote run results are
reported by GitHub separately from this local verification record.
