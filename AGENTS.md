# Repository engineering rules

- Keep this project API-only. Handlers extract/authorize inputs and delegate to
  services; services depend on repository traits rather than SQLx pools.
- Support PostgreSQL and MySQL for every persistence change. Keep driver-specific
  SQL in adapters. Verify default, postgres-only, and mysql-only builds.
- All SQL values are bound parameters. Never interpolate user-controlled IDs,
  search strings, sort expressions, or table names into SQL.
- Store timestamps in UTC and return UTC RFC3339 timestamps. MySQL DATETIME
  conversion and binary-collation text decoding belong in the adapter.
- Normalize DTO text before validation. Do not normalize passwords. Reject
  privileged/unknown fields instead of mass assignment.
- Do not serialize/debug/log passwords, password hashes, JWTs, DSNs, SMTP
  credentials, or raw request payloads. Validation errors must omit rejected values.
- New users are role `user`. Authorization reads the current database record.
  Add explicit policy checks for new endpoints; authentication alone is not authorization.
- Preserve the user+welcome-job transaction. Domain writes and associated durable
  jobs must commit atomically when the business operation requires both.
- Queue handlers must tolerate at-least-once execution. ACK/retry must preserve
  claim-token and unexpired-lease guards. Use safe error categories in job records.
- Keep CPU-heavy password operations in bounded blocking tasks, with permits
  retained until the actual task completes, even when an HTTP waiter is canceled.
- Add new migration versions to both database directories. Do not edit applied
  migrations. Update readiness compatibility policy deliberately when changing rollout behavior.
- Prefer small modules and explicit types; use `AppResult` for application paths
  and `anyhow::Result` for bootstrap/process boundaries. No unsafe code.
- Run `make check` before proposing changes. Run both real-database suites when
  changing SQL, migrations, auth/repository integration, or queue behavior. Report
  unrun checks honestly; ignored tests are not evidence of integration success.
- Never commit `.env`, generated credentials, build outputs, or local test data.
  Preserve `Cargo.lock` and the pinned toolchain for reproducible builds.
