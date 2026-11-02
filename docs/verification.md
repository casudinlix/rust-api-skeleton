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
test counts at that stage. Later extensions have separate records below.

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

At the cache-extension stage, Redis queue transport, SQL outbox relay and
Redis-backed worker dispatch were not yet implemented. That scope is superseded
by the queue-extension record below. No user, role, password hash or JWT is
automatically cached.

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

## Redis queue and transactional outbox — 1 November 2026

This record covers the optional Apalis Redis transport, PostgreSQL/MySQL outbox,
shared queue facade, configurable concurrency, relay process and deployment
documentation added after baseline `171b842`. Database queue remains the default.
The earlier sections are historical verification records, not current test counts.

Environment: Rust **1.99.1**, SQLx **0.8.6**, Apalis **1.0.0**, locally patched
Apalis Redis **1.0.0**, redis-rs **1.8.1**, Redis server **8.0.5**, PostgreSQL
**16.15**, and MySQL **8.0.46**. The Redis queue test server was separate from
the cache server and used AOF `everysec` with `noeviction`. All credentials,
databases and namespaces used for these checks were disposable test fixtures.

### Executed checks

| Check | Final result |
|---|---|
| `make check`: format and Clippy with warnings denied | Passed |
| Library unit tests | 75 passed; the 6 live Redis transport cases were executed separately |
| Artisan unit tests | 35 passed |
| HTTP, Artisan CLI and Make wrapper contracts | 19 passed |
| Rustdoc example | 1 passed (compilation) |
| PostgreSQL-only and MySQL-only all-target compilation | Both passed |
| Real PostgreSQL repository/API/database-queue/outbox suites | 4 suites passed |
| Real MySQL repository/API/database-queue/outbox suites | 4 suites passed |
| Redis cache regressions | 7 passed |
| Live Redis transport regressions | 6 passed |
| Redis worker/outbox E2E with PostgreSQL and MySQL | Both passed |
| Generated scaffold fixture with both drivers, PostgreSQL-only and MySQL-only | Passed; vendored dependency copied and compiled offline |
| `cargo build --locked --release --bins` | Passed, including the new relay binary |
| Native API + worker + standalone relay + scheduler | Passed; all four processes exited 0 on SIGTERM |
| Compose/CI YAML parsing and relative documentation links | Passed |

`make check` runs 129 tests without external services plus the compiled rustdoc
example. Its ignored network tests were also executed explicitly, using the
compiled test binaries with `--ignored --test-threads=1` and isolated database/
Redis fixtures. These correspond to `make test-postgres`, `make test-mysql`,
`make test-redis`, `make test-redis-transport`, `make test-queue-postgres` and
`make test-queue-mysql`. Ignored status in the default run is not counted as a pass.
The separate ignored scaffold test also passed. Total executed unique named tests
for this revision: **153**, plus the compiled rustdoc example. Extra reruns of
the same cases are not counted again.

### Database and recovery behavior

- Registration through real `Settings`/`AppState` selects the configured driver;
  Redis mode inserts the user and outbox in one transaction, without a Redis
  network operation. A welcome-job dedupe conflict rolls back the user insert.
- New outbox and database job creation paths are checked for UUIDv7. Application
  identity stays stable across retries and duplicate transport deliveries.
- Relay reservations, uncertain publish outcomes, lost publisher recovery,
  expired execution leases, concurrent delivery, stale/fabricated tokens,
  retry delay, permanent failure and final-attempt crash recovery are exercised.
- Completed jobs are not executed again. Outbox retention deletes 1,000 then
  the remaining 5 old completed records, while preserving failed records.
- Redis unavailability does not prevent SQL registration/outbox durability;
  worker cancellation is bounded and a later healthy worker processes the backlog.
- A Redis worker leaves the legacy SQL backlog available. A worker explicitly
  configured for the database backend then drains those jobs.
- SQL query/index candidates were measured on 50,000-row disposable fixtures
  before runtime implementation. See [the design record](design/redis-queue.md)
  for both engines' actual plans, data distribution and limits of the measurements.

### Failures found and corrected

1. **Execution-claim contention.** E2E on PostgreSQL exposed a race between the
   relay's publish-completion UPDATE and the worker's point lookup. `SKIP LOCKED`
   treated a brief publisher lock as a duplicate delivery, delaying processing
   until the next relay lease. Point lookups now use `FOR UPDATE` with a one-second
   deadline covering acquire through commit; the candidate-scanning relay still
   uses `SKIP LOCKED`. A regression holds the row lock, confirms the claim waits,
   releases the lock, then verifies immediate ownership at attempt 1. The E2E
   deadline was not increased. PostgreSQL also passed two extra repetitions after
   the fix, and both engines passed again on the final source.
2. **MySQL cancellation and pool reuse.** A one-slot pool regression showed that
   graceful connection closure could wait behind a blocked MySQL operation.
   An unfinished claim now detaches and drops its raw connection; successful
   commits retain pool reuse. The unchanged regression verifies the one-second
   claim timeout, a successful pool ping before the blocking transaction is
   released, and normal claim/ACK afterward on both engines.
3. **Unbounded upstream Redis work.** Apalis Redis 1.0.0 promotes all due scheduled
   deliveries and scans all expired workers in its original Lua hot paths.
   The local patch bounds scheduled promotion and resumable orphan recovery by
   configured batch size. A 10,240-delivery test failed with the original fetch
   script, then processed every delivery after the patch. Separate regressions
   prove partial recovery remains discoverable and active workers are preserved.
   [PATCHES.md](../vendor/apalis-redis/PATCHES.md) records source provenance,
   licensing, the exact three-file upstream diff and the patch removal criteria.

Transport checks also verify the versioned/bounded envelope, fresh internal
delivery IDs for repeated application UUIDs, invalid-envelope disposal,
concurrency limits, current-job drain, bounded terminal cleanup, and sanitized
authentication/connection failures. No test performs a global Redis flush.

### Native process smoke test

A fresh PostgreSQL database was migrated with the real `migrate` binary. The test
started separate `api`, `worker`, `relay` and `scheduler` binaries, with the
worker's bundled relay disabled. HTTP registration returned 201 and authenticated
profile lookup returned 200. SQL then contained:

```text
maintenance|completed|1|7
send_welcome_email|completed|1|7
```

The final field is the UUID version; no legacy `jobs` record was inserted.
All four processes received SIGTERM and exited with code 0. The test removed its
database and only its own Redis namespace/catalogue entry. Email used the explicit
log simulation, so no real email was sent.

### Scope and remaining limits

- The local MySQL runtime was 8.0.46; Compose/CI specify 8.4, which was not run here.
- Docker was unavailable, so the image build/run and `docker compose config`
  were not executed. YAML parsing and native compilation do not substitute for them.
- A live Redis TLS handshake, Sentinel/Cluster discovery, SMTP delivery, failover
  infrastructure and production load were not tested. The adapter exposes a
  standalone Redis endpoint; TLS URL validation is covered.
- SQL is still the authoritative ledger in Redis mode. Transport deduplication
  and lease fencing do not guarantee exactly-once external side effects.
- The new table follows the named UUIDv7 PK convention. Generic legacy PK column
  names, old stored IDs and the wider legacy items listed in the system-design
  rules were not silently migrated by this feature.
- Local verification is the acceptance evidence. GitHub Actions availability or
  quota is not used as the completion gate, and no production deployment was made.

## Existing example rule alignment — 2026-11-01 UTC

This verification covers the changes after baseline `260adcb`: named user/job
primary keys across the current examples and generators, UUIDv7 creation guards,
public user projections, versioned cursors, and bounded indexed queue maintenance.
The paired new migration is `20261102000000_align_example_primary_keys.sql`.
The six previously committed migration files are unchanged.

### Final checks

Final application checks used Rust 1.99.1, PostgreSQL 16.15, MySQL 8.0.46, and
Redis 8.0.5 on local disposable databases/namespaces. After detecting stale local
artifacts containing an earlier migration candidate, the package artifacts were
removed with `cargo clean -p rust-api-skeleton` and the final checks below were
run with `CARGO_INCREMENTAL=0`. Each resulting database/queue test executable was
also checked to contain the final guarded MySQL migration before integration
execution. No migration ledger or checksum was modified to bypass readiness.

| Check | Result |
| --- | --- |
| `make check` | Passed: formatting, all-feature clippy with warnings denied, 133 nonignored tests, one rustdoc example, PostgreSQL-only and MySQL-only all-target builds |
| `make test-postgres` | Passed all five suites: API, users, database queue, outbox, schema upgrade |
| `make test-mysql` | Passed the same five suites on the final migration |
| `make test-queue-postgres` | Passed real registration/outbox/Redis/worker integration |
| `make test-queue-mysql` | Passed the equivalent Redis worker integration |
| `make test-scaffolds` | Passed fresh generated-project formatting, clippy, a running model-factory contract test, and both single-driver builds |
| `git diff --check` and local documentation-link validation | Passed |
| Representative query plans | Accepted on 50,000 users and 50,000 jobs per engine; final MySQL generated-expression fix remeasured |

Ignored tests in the default run are not counted as executed integration tests.
The dedicated cache and transport-only suites were not rerun for this change;
those implementations are unchanged. The Redis end-to-end suites above do run
real delivery and worker execution with both database drivers. Scaffold checks
ran against a separate generated fixture; its model-factory test verifies actual
UUID versions and values rather than only matching source strings.

### Behavior established

- Upgrading the three historical migrations preserves every tested user/job
  value, including UUIDv4 and UUIDv7 identifiers, password hashes, payloads,
  deduplication keys, status, attempts, lease tokens, and microsecond timestamps.
  Migration checksums remain unchanged; rerunning migrations succeeds and the
  final schema has `users.user_id`, `jobs.job_id`, and `job_outbox.job_outbox_id`.
- A historical UUIDv4 job is claimed and completed through `DatabaseQueue` with
  its original identifier. Fabricated tokens and repeated ACKs are rejected.
  Legacy user UUIDs also remain valid JWT subjects.
- New users, welcome jobs, standalone jobs, and outbox rows use UUIDv7. Direct
  repository creation rejects invalid new UUID versions before data queries.
  Registration rejects caller-supplied `id` and `user_id` fields.
- Public user JSON exposes `user_id`. List/detail/profile and create/update
  result projections omit credential material. Only the login repository
  projection selects `password_hash`; its type cannot be serialized.
- Scoped SQLx instrumentation observes exactly one data query for each first and
  next user page at limits 1, 20, and 100, with at most `limit + 1` returned rows.
  Public lookup/contact, create/update/delete, and invalid-ID budgets are checked
  separately. No optional cache hides queries from these assertions.
- Cursor tests cover version/resource/sort/scope, malformed/unknown fields,
  timestamp precision, tied timestamps, concurrent inserts/deletes, and reset of
  the former unversioned format. Authorization still reads the current DB role.
- Claim order follows actual eligibility and deterministic PK ties. Retention
  deletes 1,000 then four unlocked old rows, skips a locked fifth remainder, and
  deletes that row after unlocking. Recent, pending, and failed records survive.
- Generated Customer, Company, Person, Status, and Uuid models produce named
  UUIDv7 keys from both `new()` and `Default`. Explicit singular names preserve
  irregular table mappings and entities ending in `s`; repository parameters
  follow the same names.

### MySQL regression resolved before completion

The first MySQL integration attempt exposed error 1292 when an omitted
`available_at` default was observed as a zero datetime during an early generated
column evaluation. Changing to a virtual generated column or an expression
DEFAULT did not resolve it. The final MySQL expression guards its branches with
`YEAR(...) > 0`; after defaults are filled, the stored `claimable_at` matches the
final timestamp exactly. Strict mode stays enabled, and explicit zero dates
remain rejected. SQLx prepared-statement regressions verify omitted values,
explicit DEFAULT, explicit microseconds, and running/retry transitions. The
related upstream report and final query plans are in the
[design record](design/example-rule-alignment.md).

A schema-test metadata decoder also needed to read MySQL's binary text wire
values as bytes and validate UTF-8, consistent with the application's adapter.
This was a test-reader fix; column types/collations were preserved.

### Compatibility and limits

User JSON changes from `id` to `user_id`; clients must update field access and
restart old cursors. Existing UUID values, JWT subjects, job payload references,
and deduplication keys are preserved. Apply the new migration and binaries in
one coordinated maintenance window: adding stored generated/indexed data may
rewrite or rebuild a populated table. MySQL's multi-statement DDL is not one
transaction; the design record documents forward recovery and rollback needs.

Measurements establish access paths and batch bounds, not production throughput
or latency guarantees. This run used MySQL 8.0.46; the configured CI/Compose 8.4
runtime was not executed here. Docker builds, live SMTP, production failover,
and production deployment were not part of this change. The prior verification
sections remain historical records, including their earlier legacy limitations;
this section records the subsequent example alignment. GitHub runner quota was
not used as an acceptance gate.


## Starter module and global response sample — 2026-11-02

The starter/reference surface was aligned with the global HTTP response facade:

- Added `examples/module_init.rs` as a minimal module showing
  `ValidatedJson<Request> -> service -> normalize_and_validate -> response helper`.
- The example uses `response::success`, `response::created`, and
  `response::cursor`; service/domain types do not depend on Axum response wrappers.
- Controller scaffolds now point developers to the compiling starter sample and
  `docs/module-init.md`.
- `artisan_scaffolds` now asserts generated requests keep the validation contract
  and generated controllers advertise the global response facade.
- README, development guide, contracts, and agent rules link to the same pattern.

This change does not touch SQL, migrations, queue semantics, or cache behavior.
The current execution environment did not provide a Rust toolchain, so
`cargo fmt`, `make check`, and `make test-scaffolds` were not executed for
this documentation/example-only alignment. The source was reviewed for consistency
against the current public helper signatures. GitHub Actions status is not used as
an acceptance gate.


## Generic process observability and readiness — 2026-11-02

This change raises the generic skeleton's operational readiness without adding
domain-specific RBAC, tenant, audit, or business tables.

Implemented:

- separate observability listeners for API, worker, scheduler and relay;
- `/health`, `/ready`, and Prometheus-text `/metrics` per process;
- exact DB migration readiness retained on every process;
- runtime readiness probe for API bind, database worker loop, Redis worker
  consumer, scheduler start, and standalone relay Redis connection;
- bounded in-memory counters/gauges for HTTP, SQLx pool state, worker execution,
  queue claim failures, Redis reconnects, relay publish and scheduler enqueue;
- Docker/Compose health checks for background processes;
- loopback-only native defaults and non-published container-network metrics ports;
- configuration validation for nonzero/distinct observability ports and APP_PORT
  collision prevention.

No SQL migration, queue schema, cache semantics, authentication model, or domain
authorization model was changed.

The current execution environment does not contain `rustc` or `cargo`
(`rustc: command not found`). Therefore formatting, Clippy, compilation,
`make check`, integration tests, Docker build/run and Compose parsing were not
executed for this revision. Source/config/documentation consistency was reviewed
against the current Axum/SQLx APIs and existing project contracts. This section
must not be read as a passing runtime verification record; rerun the standard
local verification matrix on a Rust 1.99.1 environment before tagging a release.


## Filesystem, storage link, and rotating logs — 2026-11-02

Added generic local filesystem and logging foundations:

- `LocalStorage` with private/public disks, size limit, relative-path validation,
  symlink-component rejection, atomic temporary-file rename, read/exists/delete;
- `AppState.storage` as the reusable module entrypoint;
- `storage/app/{private,public}`, `storage/logs`, and `public/assets` layout;
- `artisan storage:link` / `make storage-link`, idempotent and refusing to
  overwrite regular files/directories even with force;
- stdout/file/both logging with bounded asynchronous file writer, per-process
  filenames, never/hourly/daily rotation and bounded retained file count;
- structured custom logs through normal tracing targets and RUST_LOG filtering;
- Docker public/storage symlink plus writable `app_storage` Compose volume;
- unit/CLI/Make contracts for path validation, local storage, retention, and
  storage-link behavior.

No database schema, queue payload, authentication, cache semantics, or HTTP
business contract was changed.

The current execution environment still does not provide Rust/Cargo, so
`cargo fmt`, Clippy, compilation, `make check`, CLI tests, Docker build/run,
and Compose validation were not executed here. The source and configuration were
reviewed statically; this is not runtime evidence. Run the normal verification
matrix on Rust 1.99.1 before tagging.


## Optional Swagger/OpenAPI contract — 2026-11-02

Implemented without adding a new Cargo dependency:

- `SwaggerConfig` with runtime `SWAGGER_ENABLED`, UI path, OpenAPI path, and
  pinned HTTPS asset-base validation;
- Swagger defaults OFF; both UI and spec routes are absent when disabled;
- OpenAPI 3.1 document for all current reference API routes;
- Bearer JWT security scheme on protected operations;
- schemas/examples aligned with named `user_id`, global success/error envelopes,
  cursor `next_cursor/has_more`, request validation boundaries and safe examples;
- Swagger UI sets `persistAuthorization: false`;
- HTTP contract tests cover disabled 404 behavior, enabled UI/OpenAPI behavior,
  and verify every documented path+method maps to an actual Axum runtime route;
- configuration/unit tests reject route overlap, traversal-like paths and non-HTTPS
  asset URLs;
- README, development, operations, contracts, module-init, system rules and agent
  rules document the same contract.

No Cargo dependency or lockfile changed. The implementation deliberately avoids
adding `utoipa`/Swagger crates while this execution environment cannot regenerate
or verify `Cargo.lock`. A future library migration is allowed only with
reproducible lockfile update and route/spec drift tests.

The current execution environment still does not provide Rust/Cargo. Therefore
`cargo fmt`, Clippy, compilation, `make check`, HTTP tests and Docker build
were not executed here. Static source/config review is not runtime evidence; run
the normal Rust 1.99.1 verification matrix before tagging.


## Doctor, pre-commit, and release discipline — 2026-11-02

Added repository-level quality/release foundations:

- `artisan doctor` / `make doctor` runtime preflight;
- DB connection + exact migration readiness;
- optional Redis cache/queue connectivity checks only when enabled;
- private storage write/delete probe and public storage-link status;
- optional file-log directory write probe;
- Swagger/observability status without exposing secrets;
- versioned `.githooks/pre-commit`;
- explicit `artisan hooks:install` / `make hooks-install` with safe refusal to
  replace a pre-existing custom `core.hooksPath` unless forced;
- `make precommit` running `make check` plus scaffold verification;
- release checklist and changelog baseline.

The pre-commit gate intentionally avoids external DB/Redis requirements; runtime
dependency checks remain doctor/release concerns.

These changes were authored without executing Rust in this assistant environment.
The user's local Rust 1.99.1 environment had already demonstrated that
`make install` compiled successfully before this change. Run `make precommit`
and `make doctor` locally after pulling this revision and record the results
before creating the first release tag.


## Migration rollback/fresh and seeder tooling — 2026-11-02

Added:

- immutable forward SQLx migrations retained unchanged;
- companion rollback SQL under `rollbacks/postgres` and `rollbacks/mysql`;
- `artisan migrate:rollback --step` / `make migrate-rollback STEP=...`;
- `artisan migrate:fresh` with optional default/named seeding;
- `artisan db:seed --class` and `make db-seed CLASS=...`;
- `make:seeder` generator with explicit registry insertion;
- production force guards for destructive/write commands;
- scaffold rollback rejection;
- disposable PostgreSQL/MySQL maintenance regression wired into the live DB matrix.

SQLx 0.8.6 simple migration history was intentionally not renamed to reversible
files. Artisan constructs a temporary reversible source from unchanged forward SQL
and companion rollback SQL so historical forward checksums remain intact.

This revision still requires local execution after pull: run `make precommit`,
then `make test-postgres` and `make test-mysql` on disposable databases. MySQL
rollback deserves particular attention because DDL implicitly commits.


## Integration-test dotenv loading — 2026-11-02

Fixed the local test runner contract after real execution showed Make targets
rejecting `TEST_DATABASE_URL` even when it existed in `.env`.

- Make no longer requires test URLs to already exist in its process environment;
- integration tests load `.env` with `dotenvy` without overriding explicit CI/shell values;
- PostgreSQL prefers `TEST_POSTGRES_URL`, MySQL prefers `TEST_MYSQL_URL`;
- `TEST_DATABASE_URL` remains a backward-compatible fallback;
- Redis tests, including library transport tests, load `TEST_REDIS_URL` the same way;
- Make only exports test URLs when explicitly supplied on its command line/environment,
  avoiding an exported empty value that would mask `.env` loading.

The user-provided test output also showed the current pre-commit run stopping at
`cargo fmt --check`; that is a separate formatting gate and must be resolved with
`make fmt` before claiming the pre-commit gate passed.
