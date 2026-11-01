# Internal implementation contracts

These contracts document module boundaries and database conventions. Public
behavior is documented in README; update this file when an interface changes.

## Configuration, errors and HTTP

`config::Settings` (Clone) has `app: AppConfig`, `database: DatabaseConfig`, `cache: CacheConfig`,
`jwt: JwtConfig`, `cors: CorsConfig`, `queue: QueueConfig`,
`scheduler: SchedulerConfig`, `mail: MailConfig`.

- AppConfig: `host: String`, `port: u16`, `environment: String`, `log_format: String`,
  `request_timeout_secs: u64`, `shutdown_timeout_secs: u64`, `body_limit_bytes: usize`,
  `password_hash_concurrency: usize`, `auth_requests_per_minute: u32`.
- JwtConfig: `secret: String`, `issuer: String`, `audience: String`, `ttl_secs: u64`.
- CorsConfig: `allowed_origins: Vec<String>`.
- QueueConfig: `driver: QueueDriver` (Database|Redis), `poll_interval_ms: u64`,
  `visibility_timeout_secs: u64`, `job_timeout_secs: u64`, `max_attempts: u32`,
  `retry_base_secs: u64`, `concurrency: usize`, `redis: RedisQueueConfig`,
  `relay_enabled: bool`. Redis queue configuration is independent of cache.
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
  `cache: Arc<Cache>`, `auth_service: Arc<AuthService>`, `user_service: Arc<UserService>`,
  `queue: Arc<dyn JobQueue>`, plus HTTP auth-rate limiter as needed.

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
- `domain::models::user::User` is the public projection with `user_id: Uuid`,
  `name,email,role: String`, `created_at,updated_at: DateTime<Utc>`. It has no
  password field. `UserCredentials { user: User, password_hash: String }` is
  login-only, nonserializable, and redacts its credential in Debug.
- `UserContact` carries only user_id/name/email for welcome delivery.
- `NewUser` fields user_id/name/email/password_hash. Both adapters reject new
  IDs unless they have UUID version 7 and the RFC variant, before starting a
  transaction. Historical UUIDv4 records remain readable/updateable/deletable.
- `UserUpdate` fields name/email.
- `UserCursor` is an internal position: `created_at: DateTime<Utc>`, `user_id: Uuid`.
- `UserPage` fields `users: Vec<User>`, `next_cursor: Option<UserCursor>`.
- async-trait `UserRepository: Send + Sync`: `find_by_id(user_id:Uuid)->AppResult<Option<User>>`,
  `find_contact_by_id(user_id:Uuid)->AppResult<Option<UserContact>>`,
  `find_credentials_by_email(email:&str)->AppResult<Option<UserCredentials>>`,
  `create_with_welcome_job(user:NewUser)->AppResult<User>`,
  `list(cursor:Option<UserCursor>, limit:u32)->AppResult<UserPage>`,
  `update(user_id:Uuid, input:UserUpdate)->AppResult<User>`, `delete(user_id:Uuid)->AppResult<bool>`.
- `repositories::user_repository(pool:&DatabasePool) -> Arc<dyn UserRepository>`.
- `user_repository_with_queue(pool, QueueDriver)` selects transactional jobs or outbox insertion.
- Use descending `(created_at,user_id)` order and limit+1, with one query per
  repository page independent of page size. Public reads/writes never select the
  hash; only the login projection does. Authentication adds one live-user lookup
  per protected HTTP request and never relies on a cached role.
- Service encodes a base64url JSON envelope without padding: version=1,
  resource=users, sort=created_at_desc_user_id_desc, scope=admin:all, and position.
  Decode is bounded to 512 encoded bytes / 384 decoded bytes. Validate context,
  timestamp microsecond precision/range, UUID shape, and limit 1..100 before SQL.
  Legacy cursors return 400; clients restart at the first page.
- Final schema uses users.user_id, jobs.job_id, and job_outbox.job_outbox_id.
  `20261102000000_align_example_primary_keys.sql` renames old columns without
  changing stored UUIDs. Historical migrations and checksums stay immutable.
- Registration inserts user + jobs/outbox row in one transaction. Job insert fields:
  `job_id` (or `job_outbox_id`), `kind`, `payload`, `deduplication_key` only.
  UUID stored native PG/string MySQL. `kind='send_welcome_email'`,
  `payload={"user_id":"<UUID>"}` via SQLx Json, deduplication_key=`welcome:<uuid>`.

## Optional cache

- `config::cache::CacheDriver` None|Redis, FromStr, Display, Clone Copy.
- `CacheConfig` Clone: `driver: CacheDriver`, `redis_url: String`, `prefix: String`,
  `default_ttl_secs: u64`, `timeout_ms: u64`, `max_value_bytes: usize`.
  `validate() -> anyhow::Result<()>` never includes raw URLs in errors.
- `cache::Cache` Clone: synchronous `from_config(&CacheConfig) -> CacheResult<Self>`
  and `with_store(&CacheConfig, Arc<dyn CacheStore>) -> CacheResult<Self>`.
- Async JSON operations: `get<T>(key) -> CacheResult<Option<T>>`,
  `put<T>(key, &value, Option<Duration>) -> CacheResult<()>`,
  `forget(key) -> CacheResult<bool>`, `ping() -> CacheResult<()>`.
- Async `remember<T,E,F,Fut>(key, Option<Duration>, loader) -> Result<T,E>` uses
  cached data when available, otherwise invokes the loader and best-effort writes.
  Loader errors are propagated and never cached; concurrent misses are not locked.
- `CacheStore: Send + Sync` is the object-safe async byte backend. It receives
  prefixed keys and a read-size bound; implementations must enforce that bound.
- Redis reads are size-guarded with Lua; writes use SET with PX expiration.
  Disabled caching never opens a connection or persists values.
- Cache is opt-in, disposable data. Auth reads the live database; SQL queue behavior
  and database readiness are independent of optional Redis availability.

## Identity and services

- `domain::dtos::Normalize` trait `fn normalize(&mut self)` for request DTOs.
- `RegisterRequest` name/email/password; `LoginRequest` email/password;
  `UpdateUserRequest` name/email, with Validate, Deserialize, deny_unknown_fields.
- `UserResponse` user_id/name/email/role/created_at/updated_at with Serialize and From<User>.
- `TokenResponse` access_token/token_type/expires_in/user:UserResponse.
- `UserListQuery` Deserialize fields `cursor:Option<String>`, `limit:Option<u32>`.
- `UserListResponse` Serialize fields `items:Vec<UserResponse>`, `next_cursor:Option<String>`.
- `AuthService::new(repo:Arc<dyn UserRepository>, jwt:JwtConfig, hash_concurrency:usize)
  -> anyhow::Result<Self>`; use a precomputed Argon2id dummy hash for missing-user checks.
- Async `create_user(RegisterRequest)->AppResult<UserResponse>` for admin creation,
  `register(RegisterRequest)->AppResult<TokenResponse>`,
  `login(LoginRequest)->AppResult<TokenResponse>`, `authenticate(token:&str)->AppResult<User>`.
- `UserService::new(repo:Arc<dyn UserRepository>)->Self`;
  async `get(user_id:Uuid)->AppResult<UserResponse>`, `list(UserListQuery)->AppResult<UserListResponse>`,
  `update(user_id:Uuid,UpdateUserRequest)->AppResult<UserResponse>`, `delete(user_id:Uuid)->AppResult<()>`.
- `middleware::auth::AuthUser(pub User)` and `AdminUser(pub User)` implement
  Axum 0.8 FromRequestParts<AppState>, strict single Bearer authorization parsing.
- HS256 only; reject bad exp, issuer, audience, UUID subject; verify current user/role
  in database. Bound password hash operations using try_acquire_owned semaphore and
  spawn_blocking; permit stays INSIDE blocking task (timeout cannot free it early).
- JWT TTL integer seconds; reject unknown input fields incl role; validation must omit
  raw password/email values from error metadata. New registrations role=user.

## Queue and scheduler

- `jobs::DatabaseQueue` Clone. `new(database:DatabasePool, config:QueueConfig)->Self`.
- `queue_backend(&DatabasePool, &QueueConfig) -> Arc<dyn JobQueue>` selects SQL
  polling or Redis with a SQL outbox. `JobQueue` provides enqueue, ack,
  fail(job, reason, permanent), and prune_completed. Acquisition is backend-specific.
- async `enqueue(kind:&str, payload:serde_json::Value, deduplication_key:Option<&str>)
  ->AppResult<()>`, `claim()->AppResult<Option<ReservedJob>>`,
  `ack(job:&ReservedJob)->AppResult<bool>`, `fail(job:&ReservedJob,reason:&str)->AppResult<bool>`.
- `ReservedJob` fields public `job_id:Uuid`, `kind:String`, `payload:Value`,
  `attempts:u32`, `lock_token:Uuid`.
- `run_worker(database:DatabasePool, config:QueueConfig, mail:MailConfig,
  cancellation:CancellationToken)->anyhow::Result<()>`.
- `run_scheduler(queue:Arc<dyn JobQueue>, config:SchedulerConfig,
  cancellation:CancellationToken)->anyhow::Result<()>`.
- Jobs table fields: job_id, kind, payload, deduplication_key UNIQUE nullable,
  status (pending/running/completed/failed), attempts DEFAULT 0, available_at,
  locked_until nullable, lock_token nullable, last_error nullable,
  created_at, updated_at, generated claimable_at. Other insert fields have defaults.
  claimable_at is available_at for pending, locked_until for running, NULL for
  terminal rows; never assign it in application SQL.
- Reclaim expired running jobs until max_attempts, then mark failed. DB server UTC clock.
  Claims transaction/SKIP LOCKED; attempts increments per reservation. ACK/fail require
  matching job_id AND lock_token AND running status AND an unexpired lease. Claim
  order is claimable_at, created_at, job_id and uses jobs_claimable_idx. Up to 32
  exhausted candidates can be retired per call. Retry exponential backoff bounded.
- Welcome handler resolves user by ID and uses explicit log/smtp transport; SMTP via
  lettre TLS. Do not log email address, password, payload or SMTP credentials.
- Maintenance deletes at most 1,000 completed rows older than retention_days per
  call on either backend. It selects ordered keys with SKIP LOCKED then deletes
  those keys in the same transaction; failed rows remain for inspection. Scheduler
  dedupe key identifies UTC minute; defaults every 5 minutes.
- Redis envelopes contain only versioned job_id, mapped to job_outbox_id. SQL
  remains the source for payload/attempts/outcome; broker metadata IDs follow
  Apalis conventions. See [queue design](design/redis-queue.md).

## Verification

Run `make check` for formatting, clippy, unit/HTTP tests, rustdoc examples, and the driver matrix.
Run `make test-postgres` and `make test-mysql` with TEST_DATABASE_URL pointing to
isolated disposable databases for changes touching SQL, migrations or queue behavior.
These targets include schema_alignment, which upgrades historical v4/v7 fixtures
and verifies unchanged IDs, data, and migration checksums on both engines.
Run `make test-scaffolds` when changing generators; generated models create named
UUIDv7 identities through new()/Default and repository parameters match that name.
Run `make test-redis` with TEST_REDIS_URL for cache-driver changes; use a disposable
Redis instance and per-test namespaces, never global flush operations.
See `docs/verification.md` for the actual verification record and its limits.
