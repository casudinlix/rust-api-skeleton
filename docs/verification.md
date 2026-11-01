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
