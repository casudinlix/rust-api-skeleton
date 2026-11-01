# Existing example alignment

## Decision and scope

The user and database queue examples must demonstrate the rules in `AGENTS.md`
and `docs/system-design-rules.md`. This change renames `users.id` to `user_id` and
`jobs.id` to `job_id`, carries those names through public DTOs and SQL, separates
public user data from authentication credentials, validates new identities as
UUIDv7, and makes the existing pagination and queue maintenance queries bounded.
The Redis outbox already uses `job_outbox_id`; its primary key and durable
delivery protocol stay unchanged.

The migration is additive to the migration history, not a replacement of an
already applied file. An existing UUID, including a historical UUIDv4, is never
regenerated. JWT `sub`, welcome-job payload `user_id`, and deduplication keys keep
their original values. UUIDv4 lease/fencing tokens are not entity primary keys.

## Schema before runtime

Both drivers add `20261102000000_align_example_primary_keys.sql`. The migration:

1. Renames the two primary-key columns, retaining each value and its PK constraint.
2. Renames the existing user cursor index to
   `users_created_at_user_id_index`; its columns become
   `(created_at DESC, user_id DESC)` without a duplicate index.
3. Adds a stored generated `jobs.claimable_at` timestamp: `available_at` for a
   pending job, `locked_until` for a running job, and NULL for terminal jobs.
4. Replaces the separate ready/expired indexes with
   `jobs_claimable_idx (claimable_at, created_at, job_id)`. PostgreSQL excludes
   NULLs with a partial index; MySQL uses the same ordered columns.
5. Replaces `jobs_retention_idx` with `(status, updated_at, job_id)` so a bounded
   cleanup can select exactly its ordered candidate keys.

The generated column contains no clock call and derives only from the current
row. It follows enqueue, retry, claim, completion, and failure transitions
automatically; runtime code must not write it. PostgreSQL uses `UUID` and
`TIMESTAMPTZ`; MySQL keeps `CHAR(36) CHARACTER SET ascii COLLATE ascii_bin` and
UTC `DATETIME(6)`. Email uniqueness, role/status/attempt/lease checks, JSON
payloads, password hashes, and existing timestamp precision remain intact.

MySQL additionally guards each generated timestamp branch with `YEAR(...) > 0`.
Its automatic `CURRENT_TIMESTAMP` default can be observed as a temporary zero
datetime during an early generated-expression evaluation, matching the failure
class reported in [MySQL bug 116338](https://bugs.mysql.com/bug.php?id=116338).
The guard yields NULL only at that intermediate stage; MySQL recomputes the
stored value after filling defaults. The existing default, microsecond storage,
and strict SQL mode remain unchanged. Explicitly supplied zero dates are still
rejected. This behavior is verified with both text and prepared statements, not
assumed from the expression alone.

## Reads and writes

| Operation | Projection and predicate | Order / bound | Index and query budget |
| --- | --- | --- | --- |
| User public lookup | `user_id, name, email, role, created_at, updated_at` by `user_id` | One row | User PK; one query |
| Login lookup | Public fields plus `password_hash` by normalized `email` | One row | `users_email_unique`; one query |
| Welcome delivery lookup | `user_id, name, email` by `user_id` | One row | User PK; one query |
| First user page | Public fields | `created_at DESC, user_id DESC`, `limit + 1` | User cursor index; one query |
| Subsequent user page | Public fields and strict cursor boundary | Same order and bound | User cursor index; one query |
| Queue claim | `job_id, kind, payload, attempts`, `claimable_at <=` stable database time | `claimable_at, created_at, job_id`, one row, `FOR UPDATE SKIP LOCKED` | Claimable index; one select plus one update per candidate |
| Queue acknowledgement/failure | Update by `job_id`, matching lease token, running status, unexpired lease | One row | Jobs PK; one write |
| Completed-job retention | Select `job_id` for completed rows older than cutoff | `updated_at, job_id`, at most 1,000, `FOR UPDATE SKIP LOCKED` | Retention index; one select then one PK batch delete in the same transaction |

The queue claim order is now earliest actual eligibility (`claimable_at`) first,
with creation time and PK as deterministic ties. For a running job that means
lease expiry, rather than its original enqueue time. PostgreSQL uses
`statement_timestamp()` for the range boundary so the planner can treat it as a
stable value. MySQL uses `UTC_TIMESTAMP(6)`. The existing limit of 32 exhausted
candidate retirements per claim call remains; this is a deliberate bounded
maintenance loop, not a per-result relation query.

Retention uses a separate locked key selection and a single bounded PK delete.
A large DELETE with an ordered subquery or join is not accepted merely because
it contains LIMIT: earlier local measurements showed an unnecessary table scan
or filesort. Failed jobs are retained for investigation. Payloads are not read
for retention, and user list/contact queries never read password hashes.

Registration keeps user insertion and job/outbox insertion in one database
transaction. A new `NewUser` must contain a UUIDv7 before any transaction is
opened. Existing rows of older UUID versions remain readable, updateable, and
deletable. Queue/outbox entity constructors also generate UUIDv7; lease tokens
retain their separate random-fencing role. Query parameters remain bound.

The default database-queue registration path has a fixed data-statement budget
of two on PostgreSQL and three on MySQL; the latter includes its result read
because MySQL does not provide this INSERT RETURNING path. User update requires
one PostgreSQL statement or two MySQL statements, and deletion requires one.
Transaction control is counted separately. Scoped SQLx statement tracing in the
user integration suite verifies these write budgets and one-query public,
contact, credential, and first/next list reads at page sizes 1, 20, and 100, with
cache bypassed. A result size increase must not increase the number of relation
queries. Execution results are recorded after the real database suite runs.

## Cursor and public contract

Public user JSON exposes `user_id`. Consumers of the previous `id` field must
update in the same release. List pagination accepts a default limit of 20 and
limits 1 through 100, reads at most 101 rows, and emits a next cursor only when
the extra row exists. There is no OFFSET or total-count query.

The opaque cursor is bounded to 512 encoded bytes and contains version 1,
resource `users`, sort `created_at_desc_user_id_desc`, scope `admin:all`, and
position `(created_at, user_id)`. Every discriminator is validated. PostgreSQL
uses a row comparison on the position; MySQL uses the equivalent expanded
less-than/equality predicate. Microsecond timestamp precision and the immutable
PK tie-breaker match database storage. An old cursor is rejected and the client
must restart at the first page. A cursor is a validated position, not an
authorization credential; admin middleware still authorizes each request.

## Generator contract

The model and repository generator takes an explicit singular entity name after
name/suffix normalization. `NAME=Person` yields `person_id` for a `people` table;
`NAME=Status` yields `status_id`, not an inferred stem with its final `s` removed.
Generated model `new()`/`Default` factories use `Uuid::now_v7()`. A migration name
such as `create_people_table` alone is insufficient to infer the entity safely,
so paired migration stubs require the author to state the table-to-PK mapping,
design document, query projections, cursor order, and indexes explicitly before
implementing DDL. Generation does not silently invent a pluralization rule.

## Upgrade and rollback operations

This is a coordinated schema/code release. Stop API writers, workers, scheduler,
and relay; take the normal verified backup; run migrations; deploy binaries that
expect the new column names; then restart processes. Adding a stored generated
column may rewrite/rebuild a populated table, so measure the maintenance window
against production size first. The exact migration-ledger readiness check keeps
an old binary from treating the new schema as compatible.

PostgreSQL applies the migration in its normal SQLx transaction. MySQL DDL
implicitly commits: the users ALTER and the jobs ALTER are individually atomic,
but the whole migration is not. If the second ALTER fails after the first
commits, keep traffic stopped and recover from the verified backup or perform a
reviewed forward repair after inspecting columns, indexes, and the SQLx ledger.
Do not edit an applied migration, blindly rerun partially applied DDL, or change
ledger checksums to hide a mismatch. There is no automatic destructive down
migration. An application rollback requires the matching schema restore or a
separately reviewed compatibility migration.

## Verification and evidence

`tests/schema_alignment.rs` creates an isolated database for each driver, applies
the original migration history, inserts UUIDv4 and UUIDv7 fixtures under the old
column names, then applies the new migration with SQLx. It verifies unchanged
identities/payloads/hashes/timestamps, migration checksums, PK and index metadata,
the generated due-time transitions, and repeat migration idempotence. It also
claims and acknowledges a historical UUIDv4 job through the actual database
queue, checking unchanged identity/payload and rejection of a forged lease
token. Fresh
schema behavior and user/job integration are also covered by the existing real
database suites updated for this contract.

The schema test also inserts new pending jobs with prepared SQLx statements in
three forms: omitted `available_at`, explicit SQL `DEFAULT`, and an explicit
timestamp with six fractional digits. It checks that `claimable_at` equals the
final stored `available_at` exactly, that defaults are current UTC time, and
that the explicit microsecond value is unchanged.

Before runtime implementation, candidate SQL and this exact migration were run
against local PostgreSQL 16.15 and MySQL 8.0.46 with 50,000 users and 50,000 jobs.
Fixtures include many tied timestamps, terminal backlog, pending due/future
jobs, and running expired/unexpired leases. First/next pages at limits 1 and
100, PK/email lookups, due/empty claims, retention key selection, and bounded PK
deletion were measured. No simulated or ignored test is counted as execution
evidence.

### Measured plans

Measured on 2026-11-01 UTC using PostgreSQL 16.15 and MySQL 8.0.46, after applying
all four migrations and collecting statistics. The user fixture contains 50,000
rows with 100 users sharing each creation timestamp; the next cursor is at row
25,050, midway through a timestamp tie. The job fixture contains 40,000 completed
rows sharing one retention timestamp, 4,000 due and 1,000 future pending rows,
2,000 expired and 1,000 unexpired running rows, and 2,000 failed rows. Each job
contains a realistic welcome payload and unique deduplication key.

| Candidate | PostgreSQL actual plan | MySQL actual plan |
| --- | --- | --- |
| First page, limits 1 / 100 | Cursor index scan reads 2 / 101 rows; 0.117 / 0.092 ms | Cursor index scan reads 2 / 101 rows; 0.070 / 1.080 ms |
| Next page, limits 1 / 100 | Cursor index range reads 2 / 101 rows; 0.123 / 0.103 ms | Expanded predicate uses cursor index range, reads 2 / 101 rows; 0.392 / 0.596 ms |
| Public/contact PK lookup | `users_pkey`, one row; 0.194 / 0.062 ms | Unique constant lookup, one row |
| Login by email | `users_email_unique`, one row; 0.095 ms | Unique constant lookup, one row |
| Due claim | `jobs_claimable_idx`, one row plus row lock; 0.176 ms | Claimable index range, one row; 0.032 ms |
| No due job after moving active due times into the future in a rolled-back transaction | Claimable index range, zero rows; 0.588 ms | Claimable index range, zero rows; 0.441 ms |
| Retention key selection, limit 1,000 | `jobs_retention_idx`, exactly 1,000 rows plus locks; 0.973 ms | Covering retention index range, exactly 1,000 rows; 0.741 ms |
| Delete the selected 1,000 PKs | `jobs_pkey`, exactly 1,000 rows; 1.466 ms | `PRIMARY` range, estimated 1,000 rows; actual DELETE affected exactly 1,000 |

A follow-up measurement checked the final runtime delete shape, including the
defensive `status = 'completed'` predicate: PostgreSQL `job_id = ANY(uuid[])`
still used `jobs_pkey` for exactly 1,000 rows (7.000 ms on that run), and MySQL
`job_id IN (...)` still chose `PRIMARY`, estimating 1,000 rows. No additional
index or hint was needed.

No candidate list/claim/retention selection used a table scan or explicit sort.
The PostgreSQL empty-claim check visited 199 cached buffers after modifying
8,000 rows in its transaction; it returned zero rows and still used the due-time
range index. This is not a claim that SKIP LOCKED or MVCC makes examined rows
strictly equal to returned rows under concurrent locks/dead tuples. Routine
vacuum and real contention measurements remain operational responsibilities.

PostgreSQL figures come from `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`; MySQL read
figures come from `EXPLAIN ANALYZE`. MySQL 8.0 does not execute this DELETE through
EXPLAIN ANALYZE, so its DELETE was checked with `EXPLAIN FORMAT=JSON` and then
executed inside a rolled-back transaction. The entire MySQL select/delete/
rollback command, including client overhead, took 83.785 ms; that is not a
server-only delete latency. PK predicates for both deletes contained the 1,000
previously selected keys and no joined/subquery DELETE. These one-shot local
measurements establish access paths and bounds, not production throughput or
latency guarantees. The migration/index candidates are accepted for runtime
implementation; functional regression tests remain a separate completion gate.

### Default initialization regression and final MySQL recheck

The first API integration run exposed error 1292 when a MySQL INSERT omitted
`available_at`: the unguarded generated expression attempted to store an
intermediate zero datetime before its default was initialized. The initial plan
fixture used explicit dates and therefore did not expose this behavior. A local
minimal reproduction showed that neither changing the column to indexed
VIRTUAL nor changing the timestamp to an expression default fixed it. The final
stored expression uses the zero-year guard described above.

On MySQL 8.0.46 with its default strict modes unchanged, omitted values through
text INSERT and PREPARE/EXECUTE, explicit DEFAULT, explicit microsecond values,
running leases, and retry transitions all produced the exact expected final
timestamps. An explicit zero-date INSERT still failed on `available_at`. The
fixed, unreleased migration was then reapplied from scratch to another 50,000
user / 50,000 job fixture. Critical final plans remained:

| Final MySQL operation | Access path and actual result |
| --- | --- |
| Due claim | `jobs_claimable_idx` range, one row, 0.127 ms |
| Empty claim | Same range, zero rows, 0.197 ms |
| Retention with runtime UTC cutoff | `jobs_retention_idx` covering range, exactly 1,000 rows, 0.911 ms |
| PK batch DELETE with completed-status guard | `PRIMARY` range, estimated 1,000 rows; actual DELETE affected exactly 1,000 |

The final MySQL select/delete/rollback command including client overhead took
71.545 ms. None of these access paths gained a filesort or table scan. The
PostgreSQL migration and queries did not change. Disposable benchmark databases
were removed after recording the evidence; no migration-ledger checksum was
modified.

## References

- [PostgreSQL 16 generated columns](https://www.postgresql.org/docs/16/ddl-generated-columns.html)
- [MySQL 8.0 generated columns](https://dev.mysql.com/doc/refman/8.0/en/create-table-generated-columns.html)
- [MySQL verified generated/default datetime issue 116338](https://bugs.mysql.com/bug.php?id=116338)
