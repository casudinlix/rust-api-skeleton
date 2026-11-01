//! Isolated real-database tests for Redis delivery durability and SQL fencing.
//! Each test creates and drops only its own temporary database. No Redis server
//! is required to exercise publish/consume crash boundaries at the SQL ledger.

use std::time::Duration;

use anyhow::{Context, Result, ensure};
use rust_api_skeleton::{
    config::{
        QueueConfig,
        database::{DatabaseConfig, DatabaseDriver},
    },
    database::DatabasePool,
    errors::AppError,
    jobs::{
        ReservedJob,
        outbox::{self, OutboxDelivery, OutboxQueue},
    },
};
use serde_json::json;
use sqlx::{Row, migrate::MigrateDatabase};
use uuid::Uuid;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for a disposable PostgreSQL account with CREATE DATABASE"]
async fn postgres_outbox() -> Result<()> {
    isolated(DatabaseDriver::Postgres).await
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for a disposable MySQL account with CREATE DATABASE"]
async fn mysql_outbox() -> Result<()> {
    isolated(DatabaseDriver::Mysql).await
}

async fn isolated(driver: DatabaseDriver) -> Result<()> {
    let supplied = std::env::var("TEST_DATABASE_URL").context("set TEST_DATABASE_URL")?;
    let mut url = url::Url::parse(&supplied).context("invalid TEST_DATABASE_URL")?;
    match driver {
        DatabaseDriver::Postgres => ensure!(
            matches!(url.scheme(), "postgres" | "postgresql"),
            "PostgreSQL URL required"
        ),
        DatabaseDriver::Mysql => ensure!(url.scheme() == "mysql", "MySQL URL required"),
    }
    url.set_path(&format!("outbox_test_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::create_database(&url).await?,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::create_database(&url).await?,
        #[allow(unreachable_patterns)]
        _ => anyhow::bail!("database driver is not compiled"),
    }
    let outcome = match DatabasePool::connect(&DatabaseConfig {
        driver,
        url: url.clone(),
        max_connections: 20,
        min_connections: 0,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 30,
    })
    .await
    {
        Ok(database) => {
            let result = async {
                database.migrate().await?;
                database.ready().await?;
                exercise(&database, driver, &url).await
            }
            .await;
            database.close().await;
            result
        }
        Err(error) => Err(error),
    };
    let cleanup = match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::drop_database(&url).await,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::drop_database(&url).await,
        #[allow(unreachable_patterns)]
        _ => unreachable!("feature checked before test database creation"),
    };
    outcome?;
    cleanup.context("remove isolated outbox test database")
}

fn settings(max_attempts: u32) -> QueueConfig {
    QueueConfig {
        visibility_timeout_secs: 60,
        job_timeout_secs: 30,
        max_attempts,
        retry_base_secs: 5,
        ..QueueConfig::default()
    }
}

async fn exercise(database: &DatabasePool, driver: DatabaseDriver, url: &str) -> Result<()> {
    let queue = OutboxQueue::new(database.clone(), settings(3));
    rejects_invalid_work(&queue).await?;
    dedupe_and_uuid_are_stable(&queue, database).await?;
    relay_and_execution_fences_survive_crashes(&queue, database).await?;
    concurrent_deliveries_have_one_owner(&queue, database).await?;
    execution_waits_for_publish_lock(&queue, database).await?;
    execution_claim_timeout_releases_pool_slot(database, driver, url).await?;
    retry_and_poison_jobs_are_bounded(database).await?;
    expired_final_attempt_is_terminal(database).await?;
    transaction_helpers_preserve_atomicity(&queue, database).await?;
    retention_is_bounded_and_preserves_failed(&queue, database).await?;
    Ok(())
}

async fn delivery(queue: &OutboxQueue) -> Result<OutboxDelivery> {
    queue
        .claim_delivery()
        .await?
        .context("expected one due delivery")
}

async fn execution(queue: &OutboxQueue, id: Uuid) -> Result<ReservedJob> {
    queue
        .claim_job(id)
        .await?
        .context("expected one claimable job")
}

async fn rejects_invalid_work(queue: &OutboxQueue) -> Result<()> {
    ensure!(
        queue
            .enqueue("invalid kind", json!({}), None)
            .await
            .is_err()
    );
    ensure!(
        queue
            .enqueue("test_job", json!({}), Some("invalid/key"))
            .await
            .is_err()
    );
    ensure!(
        queue
            .enqueue("test_job", json!("x".repeat(65_536)), None)
            .await
            .is_err()
    );
    ensure!(
        queue.claim_delivery().await?.is_none(),
        "rejected enqueue persisted data"
    );
    ensure!(
        queue.claim_job(Uuid::new_v4()).await?.is_none(),
        "non-v7 identity was claimable"
    );
    Ok(())
}

async fn dedupe_and_uuid_are_stable(queue: &OutboxQueue, database: &DatabasePool) -> Result<()> {
    for _ in 0..2 {
        queue
            .enqueue("test_job", json!({"version": 1}), Some("dedupe:test"))
            .await?;
    }
    let publish = delivery(queue).await?;
    ensure!(publish.job_id.get_version_num() == 7);
    ensure!(
        queue.claim_delivery().await?.is_none(),
        "duplicate enqueue generated another row"
    );
    ensure!(queue.published(&publish).await?);
    let job = execution(queue, publish.job_id).await?;
    ensure!(job.id == publish.job_id && job.attempts == 1);
    ensure!(job.payload == json!({"version": 1}));
    ensure!(queue.ack(&job).await?);
    ensure!(
        !queue.ack(&job).await?,
        "completed job accepted another ACK"
    );
    ensure!(queue.claim_job(job.id).await?.is_none());
    queue
        .enqueue("test_job", json!({"version": 2}), Some("dedupe:test"))
        .await?;
    ensure!(
        queue.claim_delivery().await?.is_none(),
        "completion erased deduplication history"
    );
    let snapshot = state(database, job.id).await?;
    ensure!(snapshot.status == "completed" && !snapshot.active && !snapshot.relay_owned);
    Ok(())
}

async fn relay_and_execution_fences_survive_crashes(
    queue: &OutboxQueue,
    database: &DatabasePool,
) -> Result<()> {
    queue
        .enqueue("test_job", json!({}), Some("relay:crash"))
        .await?;
    let first = delivery(queue).await?;
    expire_delivery(database, first.job_id).await?;
    ensure!(
        !queue.published(&first).await?,
        "expired publisher changed delivery state"
    );
    ensure!(!queue.publish_failed(&first).await?);
    let second = delivery(queue).await?;
    ensure!(second.job_id == first.job_id && second.relay_token != first.relay_token);
    ensure!(
        !queue.published(&first).await?,
        "old publisher overrode replacement owner"
    );
    ensure!(queue.publish_failed(&second).await?);
    let snapshot = state(database, first.job_id).await?;
    ensure!(
        snapshot.attempts == 0 && snapshot.delayed && !snapshot.relay_owned,
        "publish retry changed execution attempts or lost its delay"
    );
    ensure!(queue.claim_delivery().await?.is_none());

    expire_delivery(database, first.job_id).await?;
    let third = delivery(queue).await?;
    ensure!(queue.published(&third).await?);
    ensure!(
        !queue.published(&third).await?,
        "publish acknowledgement was not fenced"
    );
    // Model an acknowledged Redis publish lost on Redis restart: SQL replays it.
    expire_delivery(database, first.job_id).await?;
    let replay = delivery(queue).await?;
    ensure!(replay.job_id == first.job_id && replay.relay_token != third.relay_token);
    let old = execution(queue, replay.job_id).await?;
    ensure!(
        !queue.published(&replay).await?,
        "late publish ACK moved active execution's recovery time"
    );
    ensure!(!queue.publish_failed(&replay).await?);
    ensure!(
        queue.claim_job(old.id).await?.is_none(),
        "duplicate message obtained an active lease"
    );

    expire_execution(database, old.id).await?;
    ensure!(
        !queue.ack(&old).await?,
        "expired execution was acknowledged"
    );
    ensure!(!queue.fail(&old, "job_timed_out").await?);
    let recovery = delivery(queue).await?;
    let current = execution(queue, recovery.job_id).await?;
    ensure!(current.id == old.id && current.lock_token != old.lock_token && current.attempts == 2);
    ensure!(
        !queue.ack(&old).await?,
        "superseded owner completed the recovered job"
    );
    ensure!(!queue.fail_permanently(&old, "invalid_job_payload").await?);
    ensure!(!queue.published(&recovery).await?);
    ensure!(queue.ack(&current).await?);
    Ok(())
}

async fn concurrent_deliveries_have_one_owner(
    queue: &OutboxQueue,
    database: &DatabasePool,
) -> Result<()> {
    queue
        .enqueue("test_job", json!({}), Some("concurrent:test"))
        .await?;
    let mut publishers = tokio::task::JoinSet::new();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(16));
    for _ in 0..16 {
        let queue = queue.clone();
        let barrier = barrier.clone();
        publishers.spawn(async move {
            barrier.wait().await;
            queue.claim_delivery().await
        });
    }
    let mut deliveries = Vec::new();
    while let Some(result) = publishers.join_next().await {
        if let Some(value) = result?? {
            deliveries.push(value);
        }
    }
    ensure!(
        deliveries.len() == 1,
        "multiple publishers owned a single outbox row"
    );
    let job_id = deliveries[0].job_id;
    let mut consumers = tokio::task::JoinSet::new();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(16));
    for _ in 0..16 {
        let queue = queue.clone();
        let barrier = barrier.clone();
        consumers.spawn(async move {
            barrier.wait().await;
            queue.claim_job(job_id).await
        });
    }
    let mut jobs = Vec::new();
    while let Some(result) = consumers.join_next().await {
        if let Some(value) = result?? {
            jobs.push(value);
        }
    }
    ensure!(
        jobs.len() == 1 && jobs[0].attempts == 1,
        "duplicate delivery executed concurrently"
    );
    ensure!(state(database, job_id).await?.attempts == 1);
    ensure!(queue.ack(&jobs[0]).await?);
    Ok(())
}

async fn execution_waits_for_publish_lock(
    queue: &OutboxQueue,
    database: &DatabasePool,
) -> Result<()> {
    queue
        .enqueue("test_job", json!({}), Some("publish:contention"))
        .await?;
    let publish = delivery(queue).await?;
    let lock = hold_execution_row(database, publish.job_id).await?;
    let claim = queue.claim_job(publish.job_id);
    tokio::pin!(claim);
    ensure!(
        tokio::time::timeout(Duration::from_millis(100), &mut claim)
            .await
            .is_err(),
        "a publisher's transient lock was treated as a duplicate delivery"
    );
    lock.commit().await?;
    let job = tokio::time::timeout(Duration::from_millis(500), claim)
        .await
        .context("claim did not resume after the publisher released its lock")??
        .context("pending job was skipped after publish contention")?;
    ensure!(job.id == publish.job_id && job.attempts == 1);
    ensure!(queue.ack(&job).await?);
    Ok(())
}

async fn execution_claim_timeout_releases_pool_slot(
    database: &DatabasePool,
    driver: DatabaseDriver,
    url: &str,
) -> Result<()> {
    let single_slot = DatabasePool::connect(&DatabaseConfig {
        driver,
        url: url.to_owned(),
        max_connections: 1,
        min_connections: 0,
        acquire_timeout_secs: 2,
        idle_timeout_secs: 30,
    })
    .await?;
    let result = async {
        let queue = OutboxQueue::new(single_slot.clone(), settings(3));
        queue
            .enqueue("test_job", json!({}), Some("claim:deadline"))
            .await?;
        let publish = delivery(&queue).await?;
        let lock = hold_execution_row(database, publish.job_id).await?;
        let outcome = tokio::time::timeout(Duration::from_secs(2), queue.claim_job(publish.job_id))
            .await
            .context("execution claim exceeded its deadline")?;
        ensure!(
            matches!(outcome, Err(AppError::Unavailable)),
            "a contended claim did not report its bounded timeout"
        );
        // The blocker is deliberately still held. Reusing a connection with
        // its old query in flight would exhaust this one-slot pool.
        tokio::time::timeout(Duration::from_secs(1), single_slot.ping())
            .await
            .context("timed-out claim kept the only SQL pool slot")??;
        ensure!(state(database, publish.job_id).await?.attempts == 0);
        lock.commit().await?;
        let job = execution(&queue, publish.job_id).await?;
        ensure!(job.attempts == 1);
        ensure!(queue.ack(&job).await?);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    single_slot.close().await;
    result
}

enum HeldExecutionRow {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::Transaction<'static, sqlx::Postgres>),
    #[cfg(feature = "mysql")]
    Mysql(sqlx::Transaction<'static, sqlx::MySql>),
}

impl HeldExecutionRow {
    async fn commit(self) -> Result<()> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(tx) => tx.commit().await?,
            #[cfg(feature = "mysql")]
            Self::Mysql(tx) => tx.commit().await?,
        }
        Ok(())
    }
}

async fn hold_execution_row(database: &DatabasePool, job_id: Uuid) -> Result<HeldExecutionRow> {
    Ok(match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut tx = pool.begin().await?;
            sqlx::query("SELECT job_outbox_id FROM job_outbox WHERE job_outbox_id = $1 FOR UPDATE")
                .bind(job_id)
                .fetch_one(&mut *tx)
                .await?;
            HeldExecutionRow::Postgres(tx)
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut tx = pool.begin().await?;
            sqlx::query("SELECT job_outbox_id FROM job_outbox WHERE job_outbox_id = ? FOR UPDATE")
                .bind(job_id.to_string())
                .fetch_one(&mut *tx)
                .await?;
            HeldExecutionRow::Mysql(tx)
        }
    })
}

async fn retry_and_poison_jobs_are_bounded(database: &DatabasePool) -> Result<()> {
    let queue = OutboxQueue::new(database.clone(), settings(2));
    queue
        .enqueue("test_job", json!({}), Some("retry:test"))
        .await?;
    let publish = delivery(&queue).await?;
    let first = execution(&queue, publish.job_id).await?;
    ensure!(
        queue
            .fail(&first, "smtp://private:secret@example.com")
            .await?
    );
    let snapshot = state(database, first.id).await?;
    ensure!(snapshot.status == "pending" && snapshot.delayed && snapshot.unavailable);
    ensure!(
        snapshot.last_error.as_deref() == Some("job_failed"),
        "raw failure text was persisted"
    );
    ensure!(
        queue.claim_job(first.id).await?.is_none(),
        "retry skipped its availability deadline"
    );
    ensure!(
        queue.claim_delivery().await?.is_none(),
        "relay skipped retry backoff"
    );
    make_available(database, first.id).await?;
    let second = execution(&queue, first.id).await?;
    ensure!(second.attempts == 2);
    ensure!(queue.fail(&second, "smtp_delivery_failed").await?);
    let snapshot = state(database, first.id).await?;
    ensure!(snapshot.status == "failed" && !snapshot.active && !snapshot.relay_owned);
    ensure!(queue.claim_job(first.id).await?.is_none());
    ensure!(queue.claim_delivery().await?.is_none());

    queue
        .enqueue("unknown_kind", json!({}), Some("poison:test"))
        .await?;
    let publish = delivery(&queue).await?;
    let poison = execution(&queue, publish.job_id).await?;
    ensure!(
        queue
            .fail_permanently(&poison, "unsupported_job_kind")
            .await?
    );
    let snapshot = state(database, poison.id).await?;
    ensure!(snapshot.status == "failed" && snapshot.attempts == 1 && !snapshot.active);
    ensure!(snapshot.last_error.as_deref() == Some("unsupported_job_kind"));
    Ok(())
}

async fn expired_final_attempt_is_terminal(database: &DatabasePool) -> Result<()> {
    let queue = OutboxQueue::new(database.clone(), settings(1));
    queue
        .enqueue("test_job", json!({}), Some("final:crash"))
        .await?;
    let publish = delivery(&queue).await?;
    let last = execution(&queue, publish.job_id).await?;
    expire_execution(database, last.id).await?;
    ensure!(
        queue.claim_delivery().await?.is_none(),
        "exhausted job was republished forever"
    );
    let snapshot = state(database, last.id).await?;
    ensure!(snapshot.status == "failed" && !snapshot.active);
    ensure!(snapshot.last_error.as_deref() == Some("attempt_limit_reached"));
    ensure!(!queue.ack(&last).await?);
    Ok(())
}

async fn transaction_helpers_preserve_atomicity(
    queue: &OutboxQueue,
    database: &DatabasePool,
) -> Result<()> {
    let user_id = Uuid::now_v7();
    let payload = json!({"user_id": user_id});
    let email = format!("outbox-{user_id}@example.com");
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO users (id, name, email, password_hash) VALUES ($1, 'Test', $2, 'fixture')")
                .bind(user_id).bind(&email).execute(&mut *tx).await?;
            outbox::insert_postgres(
                &mut tx,
                "send_welcome_email",
                &payload,
                Some("rollback:test"),
            )
            .await?;
            tx.rollback().await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO users (id, name, email, password_hash) VALUES (?, 'Test', ?, 'fixture')")
                .bind(user_id.to_string()).bind(&email).execute(&mut *tx).await?;
            outbox::insert_mysql(
                &mut tx,
                "send_welcome_email",
                &payload,
                Some("rollback:test"),
            )
            .await?;
            tx.rollback().await?;
        }
    }
    ensure!(user_count(database, user_id).await? == 0);
    ensure!(
        queue.claim_delivery().await?.is_none(),
        "rollback left an outbox record"
    );
    queue
        .enqueue(
            "send_welcome_email",
            payload.clone(),
            Some("collision:test"),
        )
        .await?;
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO users (id, name, email, password_hash) VALUES ($1, 'Test', $2, 'fixture')")
                .bind(user_id).bind(&email).execute(&mut *tx).await?;
            ensure!(
                outbox::insert_postgres(
                    &mut tx,
                    "send_welcome_email",
                    &payload,
                    Some("collision:test")
                )
                .await
                .is_err(),
                "strict domain helper ignored a dedupe conflict"
            );
            tx.rollback().await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO users (id, name, email, password_hash) VALUES (?, 'Test', ?, 'fixture')")
                .bind(user_id.to_string()).bind(&email).execute(&mut *tx).await?;
            ensure!(
                outbox::insert_mysql(
                    &mut tx,
                    "send_welcome_email",
                    &payload,
                    Some("collision:test")
                )
                .await
                .is_err(),
                "strict domain helper ignored a dedupe conflict"
            );
            tx.rollback().await?;
        }
    }
    ensure!(
        user_count(database, user_id).await? == 0,
        "failed outbox insert left a committed user"
    );
    let publish = delivery(queue).await?;
    let existing = execution(queue, publish.job_id).await?;
    ensure!(queue.ack(&existing).await?);

    // The successful strict creation path must also generate an application
    // UUIDv7 and commit the domain record and its delivery together.
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO users (id, name, email, password_hash) VALUES ($1, 'Test', $2, 'fixture')")
                .bind(user_id).bind(&email).execute(&mut *tx).await?;
            outbox::insert_postgres(
                &mut tx,
                "send_welcome_email",
                &payload,
                Some("committed:test"),
            )
            .await?;
            tx.commit().await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO users (id, name, email, password_hash) VALUES (?, 'Test', ?, 'fixture')")
                .bind(user_id.to_string()).bind(&email).execute(&mut *tx).await?;
            outbox::insert_mysql(
                &mut tx,
                "send_welcome_email",
                &payload,
                Some("committed:test"),
            )
            .await?;
            tx.commit().await?;
        }
    }
    ensure!(user_count(database, user_id).await? == 1);
    let committed = delivery(queue).await?;
    ensure!(committed.job_id.get_version_num() == 7);
    let job = execution(queue, committed.job_id).await?;
    ensure!(job.payload == payload);
    ensure!(queue.ack(&job).await?);
    Ok(())
}

async fn retention_is_bounded_and_preserves_failed(
    queue: &OutboxQueue,
    database: &DatabasePool,
) -> Result<()> {
    let ids: Vec<_> = (0..1_005).map(|_| Uuid::now_v7()).collect();
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
                "INSERT INTO job_outbox (job_outbox_id, kind, payload, status, relay_due_at, updated_at) ",
            );
            query.push_values(&ids, |mut row, id| {
                row.push_bind(*id)
                    .push_bind("retention_fixture")
                    .push_bind(sqlx::types::Json(json!({})))
                    .push_bind("completed")
                    .push("NULL")
                    .push("statement_timestamp() - INTERVAL '10 days'");
            });
            query.build().execute(pool).await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut query = sqlx::QueryBuilder::<sqlx::MySql>::new(
                "INSERT INTO job_outbox (job_outbox_id, kind, payload, status, relay_due_at, updated_at) ",
            );
            query.push_values(&ids, |mut row, id| {
                row.push_bind(id.to_string())
                    .push_bind("retention_fixture")
                    .push_bind(sqlx::types::Json(json!({})))
                    .push_bind("completed")
                    .push("NULL")
                    .push("DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 10 DAY)");
            });
            query.build().execute(pool).await?;
        }
    }
    ensure!(queue.prune_completed(0).await.is_err());
    ensure!(
        queue.prune_completed(7).await? == 1_000,
        "retention was not bounded to one batch"
    );
    ensure!(queue.prune_completed(7).await? == 5);
    ensure!(
        queue.prune_completed(7).await? == 0,
        "recent completed jobs were removed"
    );
    let failed = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(job_outbox_id) AS failed_count FROM job_outbox WHERE status = 'failed'",
        )
        .fetch_one(pool)
        .await?,
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(job_outbox_id) AS failed_count FROM job_outbox WHERE status = 'failed'",
        )
        .fetch_one(pool)
        .await?,
    };
    ensure!(failed == 3, "failed jobs were erased by maintenance");
    Ok(())
}

struct State {
    status: String,
    attempts: i64,
    active: bool,
    delayed: bool,
    unavailable: bool,
    relay_owned: bool,
    last_error: Option<String>,
}

async fn state(database: &DatabasePool, id: Uuid) -> Result<State> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let row = sqlx::query(
                "SELECT status, attempts, last_error, relay_due_at IS NOT NULL AS active, \
                 COALESCE(relay_due_at > clock_timestamp(), FALSE) AS is_delayed, \
                 available_at > clock_timestamp() AS unavailable, relay_token IS NOT NULL AS relay_owned \
                 FROM job_outbox WHERE job_outbox_id = $1",
            ).bind(id).fetch_one(pool).await?;
            Ok(State {
                status: row.try_get("status")?,
                attempts: row.try_get("attempts")?,
                active: row.try_get("active")?,
                delayed: row.try_get("is_delayed")?,
                unavailable: row.try_get("unavailable")?,
                relay_owned: row.try_get("relay_owned")?,
                last_error: row.try_get("last_error")?,
            })
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let row = sqlx::query(
                "SELECT status, attempts, last_error, relay_due_at IS NOT NULL AS active, \
                 COALESCE(relay_due_at > UTC_TIMESTAMP(6), FALSE) AS is_delayed, \
                 available_at > UTC_TIMESTAMP(6) AS unavailable, relay_token IS NOT NULL AS relay_owned \
                 FROM job_outbox WHERE job_outbox_id = ?",
            ).bind(id.to_string()).fetch_one(pool).await?;
            let status: Vec<u8> = row.try_get("status")?;
            let last_error: Option<Vec<u8>> = row.try_get("last_error")?;
            Ok(State {
                status: String::from_utf8(status)?,
                attempts: row.try_get("attempts")?,
                active: row.try_get::<i64, _>("active")? != 0,
                delayed: row.try_get::<i64, _>("is_delayed")? != 0,
                unavailable: row.try_get::<i64, _>("unavailable")? != 0,
                relay_owned: row.try_get::<i64, _>("relay_owned")? != 0,
                last_error: last_error.map(String::from_utf8).transpose()?,
            })
        }
    }
}

async fn expire_delivery(database: &DatabasePool, id: Uuid) -> Result<()> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE job_outbox SET relay_due_at = statement_timestamp() - INTERVAL '1 second' WHERE job_outbox_id = $1")
                .bind(id).execute(pool).await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE job_outbox SET relay_due_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE job_outbox_id = ?")
                .bind(id.to_string()).execute(pool).await?;
        }
    }
    Ok(())
}

async fn expire_execution(database: &DatabasePool, id: Uuid) -> Result<()> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE job_outbox SET locked_until = statement_timestamp() - INTERVAL '1 second', relay_due_at = statement_timestamp() - INTERVAL '1 second' WHERE job_outbox_id = $1")
                .bind(id).execute(pool).await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE job_outbox SET locked_until = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND), relay_due_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE job_outbox_id = ?")
                .bind(id.to_string()).execute(pool).await?;
        }
    }
    Ok(())
}

async fn make_available(database: &DatabasePool, id: Uuid) -> Result<()> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE job_outbox SET available_at = statement_timestamp() - INTERVAL '1 second', relay_due_at = statement_timestamp() - INTERVAL '1 second' WHERE job_outbox_id = $1")
                .bind(id).execute(pool).await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE job_outbox SET available_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND), relay_due_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE job_outbox_id = ?")
                .bind(id.to_string()).execute(pool).await?;
        }
    }
    Ok(())
}

async fn user_count(database: &DatabasePool, id: Uuid) -> Result<i64> {
    Ok(match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query_scalar("SELECT COUNT(id) AS user_count FROM users WHERE id = $1")
                .bind(id)
                .fetch_one(pool)
                .await?
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query_scalar("SELECT COUNT(id) AS user_count FROM users WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(pool)
                .await?
        }
    })
}
