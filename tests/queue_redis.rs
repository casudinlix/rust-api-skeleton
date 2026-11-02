//! Real API/repository -> SQL outbox -> Redis -> worker integration.
//!
//! Each invocation owns a fresh database and Redis prefix. Tests never flush
//! Redis or modify the database named by TEST_DATABASE_URL. The supplied SQL
//! account must be allowed to create and drop a disposable database.

mod support;

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use apalis_redis::Config as ApalisConfig;
use rust_api_skeleton::{
    config::{QueueConfig, QueueDriver, RedisQueueConfig, Settings, database::DatabaseDriver},
    database::DatabasePool,
    domain::{dtos::RegisterRequest, models::user::NewUser},
    jobs::{DatabaseQueue, outbox::OutboxQueue, redis::RedisTransport, run_worker},
    repositories,
    state::AppState,
};
use serde_json::json;
use sqlx::{Row, migrate::MigrateDatabase};
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const WAIT_LIMIT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const LEGACY_KEY: &str = "integration:legacy-drain";

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires disposable PostgreSQL TEST_DATABASE_URL and Redis TEST_REDIS_URL"]
async fn postgres_redis_queue() -> Result<()> {
    isolated(DatabaseDriver::Postgres).await
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires disposable MySQL TEST_DATABASE_URL and Redis TEST_REDIS_URL"]
async fn mysql_redis_queue() -> Result<()> {
    isolated(DatabaseDriver::Mysql).await
}

async fn isolated(driver: DatabaseDriver) -> Result<()> {
    let supplied = support::database_url(driver)?;
    let redis_url = support::redis_url()?;
    let mut url = url::Url::parse(&supplied).context("invalid TEST_DATABASE_URL")?;
    match driver {
        DatabaseDriver::Postgres => ensure!(
            matches!(url.scheme(), "postgres" | "postgresql"),
            "PostgreSQL test URL required"
        ),
        DatabaseDriver::Mysql => ensure!(url.scheme() == "mysql", "MySQL test URL required"),
    }
    url.set_path(&format!("redis_queue_test_{}", Uuid::now_v7().simple()));
    let database_url = url.to_string();
    let prefix = format!("queue-test:{}", Uuid::now_v7().simple());
    let settings = settings(driver, &database_url, &redis_url, &prefix)?;
    // Fail before allocating SQL fixtures when the supplied Redis is unavailable.
    redis_connection(&settings.queue.redis).await?;
    match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::create_database(&database_url).await?,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::create_database(&database_url).await?,
        #[allow(unreachable_patterns)]
        _ => bail!("database driver is not compiled"),
    }

    let outcome = match DatabasePool::connect(&settings.database).await {
        Ok(database) => {
            let result = async {
                database.migrate().await?;
                database.ready().await?;
                tokio::time::timeout(Duration::from_secs(90), exercise(&database, &settings))
                    .await
                    .context("Redis queue integration exceeded its test deadline")?
            }
            .await;
            database.close().await;
            result
        }
        Err(error) => Err(error),
    };
    // Always attempt both cleanups, even when a test assertion returned an error.
    let redis_cleanup = cleanup_redis(&settings.queue.redis).await;
    let database_cleanup = match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::drop_database(&database_url).await,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::drop_database(&database_url).await,
        #[allow(unreachable_patterns)]
        _ => unreachable!("driver checked before test database creation"),
    };
    outcome?;
    redis_cleanup?;
    database_cleanup.context("remove isolated Redis queue test database")
}

fn settings(
    driver: DatabaseDriver,
    database_url: &str,
    redis_url: &str,
    prefix: &str,
) -> Result<Settings> {
    let driver = match driver {
        DatabaseDriver::Postgres => "postgres",
        DatabaseDriver::Mysql => "mysql",
    };
    Settings::from_config(
        config::Config::builder()
            .set_override("db_driver", driver)?
            .set_override("database_url", database_url)?
            .set_override("db_max_connections", 16)?
            .set_override("db_min_connections", 0)?
            .set_override("db_acquire_timeout_secs", 5)?
            .set_override(
                "jwt_secret",
                "queue-integration-test-only-key-at-least-32-bytes",
            )?
            .set_override("cache_driver", "none")?
            .set_override("mail_transport", "log")?
            .set_override("queue_driver", "redis")?
            .set_override("queue_concurrency", 2)?
            .set_override("queue_poll_interval_ms", 100)?
            .set_override("queue_job_timeout_secs", 2)?
            .set_override("queue_visibility_timeout_secs", 12)?
            .set_override("queue_max_attempts", 3)?
            .set_override("queue_retry_base_secs", 1)?
            .set_override("queue_redis_url", redis_url)?
            .set_override("queue_redis_prefix", prefix)?
            .set_override("queue_redis_timeout_ms", 1_000)?
            .set_override("queue_relay_enabled", true)?
            .build()?,
    )
}

async fn exercise(database: &DatabasePool, settings: &Settings) -> Result<()> {
    let state = AppState::new(settings.clone(), database.clone())?;
    let welcome_key = register(&state, "registered@example.com").await?;
    let welcome = required_snapshot(database, JobTable::Outbox, &welcome_key).await?;
    ensure!(welcome.job_id.get_version_num() == 7);
    ensure!(welcome.status == "pending" && welcome.attempts == 0);
    ensure!(
        snapshot(database, JobTable::Database, &welcome_key)
            .await?
            .is_none(),
        "Redis registration also inserted a legacy SQL job"
    );
    repository_collision_rolls_back_user(database, &settings.queue).await?;

    let legacy_config = QueueConfig {
        driver: QueueDriver::Database,
        ..settings.queue.clone()
    };
    DatabaseQueue::new(database.clone(), legacy_config.clone())
        .enqueue(
            "maintenance",
            json!({"retention_days": 7}),
            Some(LEGACY_KEY),
        )
        .await?;
    let legacy = required_snapshot(database, JobTable::Database, LEGACY_KEY).await?;
    ensure!(legacy.job_id.get_version_num() == 7);
    ensure!(legacy.status == "pending" && legacy.attempts == 0);

    let worker = RunningWorker::start(database, settings, settings.queue.clone());
    let processing = async {
        let completed = wait_for_status(database, JobTable::Outbox, &welcome_key, "completed")
            .await
            .context("bundled relay did not complete registration")?;
        ensure!(completed.job_id == welcome.job_id && completed.attempts == 1);
        poison_jobs_fail_once(&state, database).await?;
        duplicate_deliveries_do_not_repeat_execution(
            &state,
            database,
            &settings.queue.redis,
            &welcome_key,
            welcome.job_id,
        )
        .await?;
        ensure!(
            required_snapshot(database, JobTable::Database, LEGACY_KEY).await? == legacy,
            "Redis worker claimed or migrated a pre-existing database job"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let stopped = worker.stop().await;
    processing?;
    stopped?;

    let worker = RunningWorker::start(database, settings, legacy_config);
    let draining = wait_for_status(database, JobTable::Database, LEGACY_KEY, "completed").await;
    let stopped = worker.stop().await;
    let drained = draining?;
    stopped?;
    ensure!(drained.job_id == legacy.job_id && drained.attempts == 1);
    ensure!(
        snapshot(database, JobTable::Outbox, LEGACY_KEY)
            .await?
            .is_none(),
        "driver override silently copied the legacy job into the outbox"
    );
    redis_outage_preserves_registration_and_cancellation(database, settings).await
}

async fn register(state: &AppState, email: &str) -> Result<String> {
    let response = state
        .auth_service
        .register(RegisterRequest {
            name: "Queue integration user".to_owned(),
            email: email.to_owned(),
            password: "public-test-password-for-registration".to_owned(),
        })
        .await?;
    ensure!(response.user.user_id.get_version_num() == 7);
    let repository = repositories::user_repository_with_queue(&state.database, QueueDriver::Redis);
    ensure!(
        repository
            .find_by_id(response.user.user_id)
            .await?
            .is_some()
    );
    Ok(format!("welcome:{}", response.user.user_id))
}

async fn repository_collision_rolls_back_user(
    database: &DatabasePool,
    config: &QueueConfig,
) -> Result<()> {
    let user_id = Uuid::now_v7();
    let key = format!("welcome:{user_id}");
    OutboxQueue::new(database.clone(), config.clone())
        .enqueue(
            "send_welcome_email",
            json!({"user_id": user_id}),
            Some(&key),
        )
        .await?;
    let before = required_snapshot(database, JobTable::Outbox, &key).await?;
    let repository = repositories::user_repository_with_queue(database, QueueDriver::Redis);
    let outcome = repository
        .create_with_welcome_job(NewUser {
            user_id,
            name: "Transaction rollback".to_owned(),
            email: "rollback@example.com".to_owned(),
            password_hash: "test-only-unused-hash".to_owned(),
        })
        .await;
    ensure!(
        outcome.is_err(),
        "outbox deduplication collision was silently accepted"
    );
    ensure!(
        repository.find_by_id(user_id).await?.is_none(),
        "user committed despite a failed transactional welcome outbox insert"
    );
    ensure!(required_snapshot(database, JobTable::Outbox, &key).await? == before);
    Ok(())
}

async fn poison_jobs_fail_once(state: &AppState, database: &DatabasePool) -> Result<()> {
    state
        .queue
        .enqueue("unsupported_kind", json!({}), Some("integration:unknown"))
        .await?;
    state
        .queue
        .enqueue(
            "send_welcome_email",
            json!({"user_id": "invalid-user-identifier", "unexpected": true}),
            Some("integration:malformed"),
        )
        .await?;
    let unknown =
        wait_for_status(database, JobTable::Outbox, "integration:unknown", "failed").await?;
    ensure!(unknown.attempts == 1 && unknown.last_error.as_deref() == Some("unsupported_job_kind"));
    let malformed = wait_for_status(
        database,
        JobTable::Outbox,
        "integration:malformed",
        "failed",
    )
    .await?;
    ensure!(
        malformed.attempts == 1 && malformed.last_error.as_deref() == Some("invalid_job_payload")
    );
    Ok(())
}

async fn duplicate_deliveries_do_not_repeat_execution(
    state: &AppState,
    database: &DatabasePool,
    config: &RedisQueueConfig,
    welcome_key: &str,
    job_id: Uuid,
) -> Result<()> {
    let transport = RedisTransport::connect(config).await?;
    transport.publish(job_id).await?;
    transport.publish(job_id).await?;
    state
        .queue
        .enqueue(
            "maintenance",
            json!({"retention_days": 7}),
            Some("integration:after-duplicates"),
        )
        .await?;
    wait_for_status(
        database,
        JobTable::Outbox,
        "integration:after-duplicates",
        "completed",
    )
    .await?;
    // SQL completion alone would not prove the duplicate messages were consumed.
    // Wait until Apalis has acknowledged and cleaned all transport payloads.
    wait_for_transport_drain(config).await?;
    let completed = required_snapshot(database, JobTable::Outbox, welcome_key).await?;
    ensure!(
        completed.job_id == job_id && completed.status == "completed" && completed.attempts == 1
    );
    Ok(())
}

async fn redis_outage_preserves_registration_and_cancellation(
    database: &DatabasePool,
    settings: &Settings,
) -> Result<()> {
    // A listening socket that never speaks Redis makes connection progress
    // observable without depending on an arbitrary unused port or sleep.
    let unavailable = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mut offline = settings.clone();
    offline.queue.redis.url = format!("redis://{}/0", unavailable.local_addr()?);
    offline.queue.redis.timeout_ms = 100;
    let state = AppState::new(offline.clone(), database.clone())?;
    let key = tokio::time::timeout(
        Duration::from_secs(10),
        register(&state, "registered-during-outage@example.com"),
    )
    .await
    .context("registration waited for unavailable Redis")??;
    let pending = required_snapshot(database, JobTable::Outbox, &key).await?;
    ensure!(pending.status == "pending" && pending.attempts == 0);
    let worker = RunningWorker::start(database, &offline, offline.queue.clone());
    let connected = tokio::time::timeout(Duration::from_secs(3), unavailable.accept())
        .await
        .context("worker did not attempt its configured Redis endpoint")?;
    // Keep the accepted socket open through shutdown to exercise cancellation
    // while Redis is unresponsive, rather than a quick connection refusal.
    let (connection, _) = connected?;
    worker.stop().await?;
    drop(connection);
    ensure!(
        required_snapshot(database, JobTable::Outbox, &key).await? == pending,
        "transport unavailability consumed a business execution attempt"
    );

    let worker = RunningWorker::start(database, settings, settings.queue.clone());
    let recovery = wait_for_status(database, JobTable::Outbox, &key, "completed").await;
    let stopped = worker.stop().await;
    let recovered = recovery.context("registration did not recover after Redis outage")?;
    stopped?;
    ensure!(recovered.job_id == pending.job_id && recovered.attempts == 1);
    Ok(())
}

struct RunningWorker {
    cancellation: CancellationToken,
    task: Option<JoinHandle<Result<()>>>,
}

impl RunningWorker {
    fn start(database: &DatabasePool, settings: &Settings, config: QueueConfig) -> Self {
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run_worker(
            database.clone(),
            config,
            settings.mail.clone(),
            cancellation.clone(),
        ));
        Self {
            cancellation,
            task: Some(task),
        }
    }

    async fn stop(mut self) -> Result<()> {
        self.cancellation.cancel();
        // Keep the handle in the Drop guard while awaiting: cancellation of the
        // test itself must not detach the worker during its shutdown.
        let task = self.task.as_mut().context("worker already stopped")?;
        match tokio::time::timeout(Duration::from_secs(6), &mut *task).await {
            Ok(result) => result.context("worker task panicked")?,
            Err(_) => {
                task.abort();
                let _ = (&mut *task).await;
                bail!("worker did not stop within the bounded drain deadline")
            }
        }
    }
}

impl Drop for RunningWorker {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Clone, Copy)]
enum JobTable {
    Database,
    Outbox,
}

#[derive(Debug, PartialEq)]
struct JobSnapshot {
    job_id: Uuid,
    status: String,
    attempts: i64,
    last_error: Option<String>,
}

async fn snapshot(
    database: &DatabasePool,
    table: JobTable,
    key: &str,
) -> Result<Option<JobSnapshot>> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let sql = match table {
                JobTable::Database => {
                    "SELECT job_id, status, attempts, last_error FROM jobs WHERE deduplication_key = $1"
                }
                JobTable::Outbox => {
                    "SELECT job_outbox_id AS job_id, status, attempts, last_error FROM job_outbox WHERE deduplication_key = $1"
                }
            };
            sqlx::query(sql)
                .bind(key)
                .fetch_optional(pool)
                .await?
                .map(|row| {
                    Ok(JobSnapshot {
                        job_id: row.try_get("job_id")?,
                        status: row.try_get("status")?,
                        attempts: row.try_get("attempts")?,
                        last_error: row.try_get("last_error")?,
                    })
                })
                .transpose()
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let sql = match table {
                JobTable::Database => {
                    "SELECT job_id, status, attempts, last_error FROM jobs WHERE deduplication_key = ?"
                }
                JobTable::Outbox => {
                    "SELECT job_outbox_id AS job_id, status, attempts, last_error FROM job_outbox WHERE deduplication_key = ?"
                }
            };
            sqlx::query(sql)
                .bind(key)
                .fetch_optional(pool)
                .await?
                .map(|row| {
                    let id: Vec<u8> = row.try_get("job_id")?;
                    let status: Vec<u8> = row.try_get("status")?;
                    let error: Option<Vec<u8>> = row.try_get("last_error")?;
                    Ok(JobSnapshot {
                        job_id: Uuid::parse_str(std::str::from_utf8(&id)?)?,
                        status: String::from_utf8(status)?,
                        attempts: row.try_get("attempts")?,
                        last_error: error.map(String::from_utf8).transpose()?,
                    })
                })
                .transpose()
        }
    }
}

async fn required_snapshot(
    database: &DatabasePool,
    table: JobTable,
    key: &str,
) -> Result<JobSnapshot> {
    snapshot(database, table, key)
        .await?
        .context("expected durable job is missing")
}

async fn wait_for_status(
    database: &DatabasePool,
    table: JobTable,
    key: &str,
    expected: &str,
) -> Result<JobSnapshot> {
    let mut poll = tokio::time::interval(POLL_INTERVAL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tokio::time::timeout(WAIT_LIMIT, async {
        loop {
            poll.tick().await;
            let current = required_snapshot(database, table, key).await?;
            if current.status == expected {
                return Ok(current);
            }
            ensure!(
                current.status != "failed" && current.status != "completed",
                "job reached an unexpected terminal state: {current:?}"
            );
        }
    })
    .await
    .with_context(|| format!("job fixture {key} did not reach durable status {expected}"))?
}

async fn redis_connection(config: &RedisQueueConfig) -> Result<redis::aio::MultiplexedConnection> {
    let timeout = Duration::from_secs(2);
    let client = redis::Client::open(config.url.as_str())
        .map_err(|_| anyhow::anyhow!("invalid test Redis settings"))?;
    let connection_config = redis::AsyncConnectionConfig::new()
        .set_connection_timeout(Some(timeout))
        .set_response_timeout(Some(timeout));
    client
        .get_multiplexed_async_connection_with_config(&connection_config)
        .await
        .map_err(|_| anyhow::anyhow!("test Redis connection failed"))
}

async fn wait_for_transport_drain(config: &RedisQueueConfig) -> Result<()> {
    let keys = ApalisConfig::default().queue(&config.prefix);
    let mut connection = redis_connection(config).await?;
    let mut poll = tokio::time::interval(POLL_INTERVAL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tokio::time::timeout(WAIT_LIMIT, async {
        loop {
            poll.tick().await;
            let (payloads, active): (u64, u64) = redis::pipe()
                .cmd("HLEN")
                .arg(keys.job_data_hash())
                .cmd("LLEN")
                .arg(keys.active_jobs_list())
                .query_async(&mut connection)
                .await?;
            if payloads == 0 && active == 0 {
                return Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await
    .context("Redis deliveries were not acknowledged and cleaned")?
}

async fn cleanup_redis(config: &RedisQueueConfig) -> Result<()> {
    // The prefix was generated by this fixture and cannot contain glob syntax.
    // SCAN follows Redis's cursor; bounded chunks remove only owned keys.
    let owned = format!("{}:", config.prefix);
    let keys = ApalisConfig::default().queue(&config.prefix);
    let mut connection = redis_connection(config).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut cursor = 0_u64;
        loop {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(format!("{owned}*"))
                .arg("COUNT")
                .arg(100)
                .query_async(&mut connection)
                .await?;
            ensure!(
                batch.iter().all(|key| key.starts_with(&owned)),
                "test cleanup escaped its namespace"
            );
            for chunk in batch.chunks(100) {
                redis::cmd("UNLINK")
                    .arg(chunk)
                    .query_async::<u64>(&mut connection)
                    .await?;
            }
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        // Apalis's shared catalogue is a sorted set of workers-set names.
        // Remove our member only, preserving registrations from other queues.
        redis::cmd("ZREM")
            .arg("core:apalis:queues:list")
            .arg(keys.workers_set())
            .query_async::<u64>(&mut connection)
            .await?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("isolated Redis queue cleanup exceeded its deadline")?
}
