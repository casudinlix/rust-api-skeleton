# Internal implementation contracts

These contracts document module boundaries and database conventions. Public
behavior is documented in README; update this file when an interface changes.

## Configuration, errors and HTTP

`config::Settings` (Clone) has `app: AppConfig`, `database: DatabaseConfig`,
`jwt: JwtConfig`, `cors: CorsConfig`, `queue: QueueConfig`,
`scheduler: SchedulerConfig`, `mail: MailConfig`.

- AppConfig: `host: String`, `port: u16`, `environment: String`, `log_format: String`,
  `request_timeout_secs: u64`, `shutdown_timeout_secs: u64`, `body_limit_bytes: usize`,
  `password_hash_concurrency: usize`, `auth_requests_per_minute: u32`.
- JwtConfig: `secret: String`, `issuer: String`, `audience: String`, `ttl_secs: u64`.
- CorsConfig: `allowed_origins: Vec<String>`.
- QueueConfig: `poll_interval_ms: u64`, `visibility_timeout_secs: u64`,
  `job_timeout_secs: u64`, `max_attempts: u32`, `retry_base_secs: u64`.
- SchedulerConfig: `maintenance_cron: String`, `retention_days: u32`.
- MailConfig: `transport: String` (log|smtp), `from: String`, `smtp_host: String`,
  `smtp_port: u16`, `smtp_username: String`, `smtp_password: String`.
- `Settings::load() -> anyhow::Result<Self>` and `validate() -> anyhow::Result<()>`.
- `errors::AppError`: BadRequest(String), Validation(serde_json::Value), Unauthorized,
  Forbidden, NotFound(String), Conflict(String), TooManyRequests, Unavailable,
  Database(sqlx::Error), Internal(anyhow::Error), with From for SQLx and anyhow.
- `type errors::AppResult<T> = Result<T, AppError>`.
- `utils::response::ApiResponse<T>` fields: success, message, data, errors;
  `ApiResponse::success(message: impl Into<String>, data: T) -> Self`.
- `utils::validator::ValidatedJson<T>(pub T)`: deserialize -> Normalize -> Validate;
  validation errors are 422, invalid JSON 400, unsupported media 415, large body 413.
- `utils::shutdown::shutdown_signal() -> impl Future<Output = ()>`.
- `state::AppState`: `settings: Arc<Settings>`, `database: DatabasePool`,
  `auth_service: Arc<AuthService>`, `user_service: Arc<UserService>`,
  `queue: Arc<DatabaseQueue>`, plus HTTP auth-rate limiter as needed.

## Persistence

- `config::database::DatabaseDriver` Postgres|Mysql, FromStr, Display, Clone Copy.
- `DatabaseConfig` Clone: `driver: DatabaseDriver`, `url: String`,
  `max_connections: u32`, `min_connections: u32`, `acquire_timeout_secs: u64`,
  `idle_timeout_secs: u64`. Methods `validate() -> anyhow::Result<()>`.
- `database::DatabasePool` Clone enum with feature-gated `Postgres(PgPool)` and
  `Mysql(MySqlPool)` variants. Async `connect(&DatabaseConfig) -> anyhow::Result<Self>`,
  `ping(&self) -> AppResult<()>`, `ready(&self) -> AppResult<()>`,
  `migrate(&self) -> anyhow::Result<()>`, `close(&self)`. Readiness validates the
  exact embedded migration history and checksum; it performs no schema writes.
- `domain::models::user::User` Clone with `id: Uuid`, `name,email,password_hash,role: String`,
  `created_at,updated_at: DateTime<Utc>`. Password hash is never serialized/debugged.
- `NewUser` fields id/name/email/password_hash. `UserUpdate` fields name/email.
- `UserCursor` serializable: `created_at: DateTime<Utc>`, `id: Uuid`.
- `UserPage` fields `users: Vec<User>`, `next_cursor: Option<String>`.
- async-trait `UserRepository: Send + Sync`: `find_by_id(id:Uuid)->AppResult<Option<User>>`,
  `find_by_email(email:&str)->AppResult<Option<User>>`,
  `create_with_welcome_job(user:NewUser)->AppResult<User>`,
  `list(cursor:Option<UserCursor>, limit:u32)->AppResult<UserPage>`,
  `update(id:Uuid, input:UserUpdate)->AppResult<User>`, `delete(id:Uuid)->AppResult<bool>`.
- `repositories::user_repository(pool:&DatabasePool) -> Arc<dyn UserRepository>`.
- Use descending `(created_at,id)` order, limit+1. Encode `UserCursor` JSON as base64url
  without padding. Service validates limit 1..100 and cursor before repository calls.
- User migration file names match the user request; jobs schema supplied by the jobs migration.
- Registration inserts user + jobs row in one transaction. Contract insert fields:
  `id, kind, payload, deduplication_key` only (other fields have defaults).
  UUID stored native PG/string MySQL. `kind='send_welcome_email'`,
  `payload={"user_id":"<UUID>"}` via SQLx Json, deduplication_key=`welcome:<uuid>`.

## Identity and services

- `domain::dtos::Normalize` trait `fn normalize(&mut self)` for request DTOs.
- `RegisterRequest` name/email/password; `LoginRequest` email/password;
  `UpdateUserRequest` name/email, with Validate, Deserialize, deny_unknown_fields.
- `UserResponse` id/name/email/role/created_at/updated_at with Serialize and From<User>.
- `TokenResponse` access_token/token_type/expires_in/user:UserResponse.
- `UserListQuery` Deserialize fields `cursor:Option<String>`, `limit:Option<u32>`.
- `UserListResponse` Serialize fields `items:Vec<UserResponse>`, `next_cursor:Option<String>`.
- `AuthService::new(repo:Arc<dyn UserRepository>, jwt:JwtConfig, hash_concurrency:usize)
  -> anyhow::Result<Self>`; use a precomputed Argon2id dummy hash for missing-user checks.
- Async `create_user(RegisterRequest)->AppResult<UserResponse>` for admin creation,
  `register(RegisterRequest)->AppResult<TokenResponse>`,
  `login(LoginRequest)->AppResult<TokenResponse>`, `authenticate(token:&str)->AppResult<User>`.
- `UserService::new(repo:Arc<dyn UserRepository>)->Self`;
  async `get(id:Uuid)->AppResult<UserResponse>`, `list(UserListQuery)->AppResult<UserListResponse>`,
  `update(id:Uuid,UpdateUserRequest)->AppResult<UserResponse>`, `delete(id:Uuid)->AppResult<()>`.
- `middleware::auth::AuthUser(pub User)` and `AdminUser(pub User)` implement
  Axum 0.8 FromRequestParts<AppState>, strict single Bearer authorization parsing.
- HS256 only; reject bad exp, issuer, audience, UUID subject; verify current user/role
  in database. Bound password hash operations using try_acquire_owned semaphore and
  spawn_blocking; permit stays INSIDE blocking task (timeout cannot free it early).
- JWT TTL integer seconds; reject unknown input fields incl role; validation must omit
  raw password/email values from error metadata. New registrations role=user.

## Queue and scheduler

- `jobs::DatabaseQueue` Clone. `new(database:DatabasePool, config:QueueConfig)->Self`.
- async `enqueue(kind:&str, payload:serde_json::Value, deduplication_key:Option<&str>)
  ->AppResult<()>`, `claim()->AppResult<Option<ReservedJob>>`,
  `ack(job:&ReservedJob)->AppResult<bool>`, `fail(job:&ReservedJob,reason:&str)->AppResult<bool>`.
- `ReservedJob` fields public `id:Uuid`, `kind:String`, `payload:Value`,
  `attempts:u32`, `lock_token:Uuid`.
- `run_worker(database:DatabasePool, config:QueueConfig, mail:MailConfig,
  cancellation:CancellationToken)->anyhow::Result<()>`.
- `run_scheduler(queue:Arc<DatabaseQueue>, config:SchedulerConfig,
  cancellation:CancellationToken)->anyhow::Result<()>`.
- Jobs table fields: id, kind, payload, deduplication_key UNIQUE nullable,
  status (pending/running/completed/failed), attempts DEFAULT 0, available_at,
  locked_until nullable, lock_token nullable, last_error nullable,
  created_at, updated_at. All non-insert fields default appropriately.
- Reclaim expired running jobs until max_attempts, then mark failed. DB server UTC clock.
  Claims transaction/SKIP LOCKED; attempts increments per reservation. ACK/fail require
  matching id AND lock_token AND running status. Retry exponential backoff bounded.
- Welcome handler resolves user by ID and uses explicit log/smtp transport; SMTP via
  lettre TLS. Do not log email address, password, payload or SMTP credentials.
- Maintenance job deletes completed rows older than retention_days; failed rows remain
  for inspection. Scheduler dedupe key identifies UTC minute; defaults every 5 minutes.

## Verification

Run `make check` for formatting, clippy, unit/HTTP tests, and the driver matrix.
Run `make test-postgres` and `make test-mysql` with TEST_DATABASE_URL pointing to
isolated disposable databases for changes touching SQL, migrations or queue behavior.
See `docs/verification.md` for the actual verification record and its limits.
