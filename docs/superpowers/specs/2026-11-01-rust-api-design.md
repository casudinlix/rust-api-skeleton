# Rust API skeleton design

## Intent and constraints

Implement the user's specified API-only Laravel-inspired Rust layout in the empty
`casudinlix/rust-api-skeleton` repository. Use Axum >=0.7, Tokio full, SQLx >=0.8,
validator, serde, config/dotenvy, jsonwebtoken, tracing, thiserror/anyhow,
tower-http, async-trait, and tokio-cron-scheduler. The requested paths remain;
additional files support migrations, tests, deployment, and reusable bootstrapping.

## Architecture and implementation choices

The maintainer requested a working implementation, so unspecified details below
are implementation defaults, not additional requirements attributed to them.

- Handlers -> services -> object-safe repository traits -> typed database pools.
- PostgreSQL and MySQL are default Cargo features. Either may be compiled alone;
  `DB_DRIVER` chooses a compiled driver at runtime and a mismatch fails on boot.
- Separate SQL dialects preserve native UUID/timestamp handling. PostgreSQL uses
  UUID/TIMESTAMPTZ; MySQL uses CHAR(36)/DATETIME(6) and UTC connections.
- Durable SQL queue, with registration and welcome-job insertion in one transaction.
  Alternatives considered: in-process channels cannot span the three binaries;
  Redis is viable but would add infrastructure not requested in this skeleton.
- Queue claims use transactions and SKIP LOCKED, visibility deadlines, owner tokens,
  bounded retries/backoff, terminal failure state, and at-least-once semantics.
  Handlers must be idempotent. SMTP cannot promise exactly-once external delivery.
- Scheduler dispatches maintenance jobs in UTC with a unique per-minute key.
- Auth uses Argon2id off the async executor, bounded hashing concurrency, HS256 JWT
  with issuer/audience/expiry checks, and database-backed user/role checks per request.
- New registrations always have role `user`. Admin CRUD requires an admin role.
  Users can view/update only their own profile through `/api/v1/me`.
- JSON envelopes, explicit CORS, request IDs, bounded request bodies and timeouts,
  JSON errors, graceful SIGTERM/SIGINT shutdown, `/health` and `/ready`.
- UTC timestamps and bounded keyset cursor pagination. Email and name normalized
  before validation; secrets and passwords excluded from responses and logs.
- Migration binary is explicit: production services do not alter schema on boot.
- Mail transport supports SMTP and an explicitly documented local log mode.

## Verification and practical limits

Run formatting, clippy, unit/HTTP tests, a compile feature matrix, and database
integration tests against PostgreSQL and MySQL when services can be provisioned.
Test transactions, duplicate email, malformed/expired JWTs, role checks, cursor
boundaries, retry/dead-letter/fencing behavior, and scheduler deduplication.
Deployment includes non-root multi-stage Docker, Compose development profiles,
Make targets and documentation. Real credentials, infrastructure load sizing,
email-domain setup, and a distributed edge rate limiter remain deployment inputs.
