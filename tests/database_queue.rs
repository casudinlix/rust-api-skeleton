//! These ignored tests create and remove an isolated database using TEST_DATABASE_URL.
//! The configured account needs CREATE DATABASE/DROP DATABASE privileges. Existing
//! databases and jobs are never truncated or modified.

mod support;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use rust_api_skeleton::{
    config::{
        QueueConfig,
        database::{DatabaseConfig, DatabaseDriver},
    },
    database::DatabasePool,
    jobs::{DatabaseQueue, ReservedJob},
};
use serde_json::json;
use sqlx::{Row, migrate::MigrateDatabase};
use std::collections::HashSet;
use uuid::Uuid;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for a PostgreSQL account able to create test databases"]
async fn postgres_queue() -> Result<()> {
    exercise_isolated_database(DatabaseDriver::Postgres).await
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for a MySQL account able to create test databases"]
async fn mysql_queue() -> Result<()> {
    exercise_isolated_database(DatabaseDriver::Mysql).await
}

async fn exercise_isolated_database(driver: DatabaseDriver) -> Result<()> {
    let supplied = support::database_url(driver)?;
    let mut url = url::Url::parse(&supplied).context("invalid TEST_DATABASE_URL")?;
    match driver {
        DatabaseDriver::Postgres => ensure!(
            matches!(url.scheme(), "postgres" | "postgresql"),
            "postgres_queue requires a PostgreSQL URL"
        ),
        DatabaseDriver::Mysql => {
            ensure!(url.scheme() == "mysql", "mysql_queue requires a MySQL URL")
        }
    }
    let name = format!("queue_test_{}", Uuid::new_v4().simple());
    url.set_path(&name);
    let url = url.to_string();
    match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::create_database(&url).await?,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::create_database(&url).await?,
        #[allow(unreachable_patterns)]
        _ => anyhow::bail!("requested database feature is not compiled"),
    }
    let configuration = DatabaseConfig {
        driver,
        url: url.clone(),
        max_connections: 16,
        min_connections: 0,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 30,
    };
    let outcome = match DatabasePool::connect(&configuration).await {
        Ok(database) => {
            let result = async {
                database.migrate().await?;
                exercise_queue(&database).await
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
        _ => unreachable!("feature checked before database creation"),
    };
    outcome?;
    cleanup.context("remove isolated queue test database")?;
    Ok(())
}

fn queue_config(max_attempts: u32) -> QueueConfig {
    QueueConfig {
        poll_interval_ms: 20,
        visibility_timeout_secs: 60,
        job_timeout_secs: 30,
        max_attempts,
        retry_base_secs: 2,
        ..QueueConfig::default()
    }
}

async fn exercise_queue(database: &DatabasePool) -> Result<()> {
    let queue = DatabaseQueue::new(database.clone(), queue_config(3));
    deduplication_survives_completion(&queue, database).await?;
    claims_follow_eligibility_and_primary_key_ties(&queue, database).await?;
    concurrent_claims_eventually_reserve_each_job_once(&queue).await?;
    expired_and_superseded_claims_cannot_finish(&queue, database).await?;
    retries_wait_and_stop_at_the_attempt_limit(&queue, database).await?;
    permanent_failure_is_fenced_and_sanitized(&queue, database).await?;
    final_attempt_expiration_becomes_terminal(database).await?;
    maintenance_keeps_recent_and_failed_jobs(&queue, database).await?;
    maintenance_is_bounded_and_skips_locked_rows(&queue, database).await?;
    Ok(())
}

async fn next(queue: &DatabaseQueue) -> Result<ReservedJob> {
    let job = queue.claim().await?.context("expected one available job")?;
    ensure!(
        job.job_id.get_version_num() == 7,
        "new job PK must be UUIDv7"
    );
    Ok(job)
}

async fn deduplication_survives_completion(
    queue: &DatabaseQueue,
    database: &DatabasePool,
) -> Result<()> {
    for invalid in ["", "non-ascii-\u{00e9}", "trailing-space "] {
        ensure!(
            queue
                .enqueue("maintenance", json!({}), Some(invalid))
                .await
                .is_err(),
            "invalid dedupe token was accepted"
        );
    }
    ensure!(
        queue.claim().await?.is_none(),
        "rejected enqueue still created a job"
    );
    let key = "maintenance:test-minute";
    queue
        .enqueue("maintenance", json!({"retention_days": 7}), Some(key))
        .await?;
    queue
        .enqueue("maintenance", json!({"retention_days": 7}), Some(key))
        .await?;
    let job = next(queue).await?;
    ensure!(
        queue.claim().await?.is_none(),
        "duplicate enqueue created a second job"
    );
    ensure!(queue.ack(&job).await?, "owner could not complete its job");
    queue
        .enqueue("maintenance", json!({"retention_days": 7}), Some(key))
        .await?;
    ensure!(
        queue.claim().await?.is_none(),
        "completion erased deduplication history"
    );

    age_job(database, job.job_id).await?;
    ensure!(
        queue.prune_completed(1).await? == 1,
        "old completed job was not pruned"
    );
    queue
        .enqueue("maintenance", json!({"retention_days": 7}), Some(key))
        .await?;
    let replacement = next(queue).await?;
    ensure!(
        replacement.job_id != job.job_id,
        "pruning did not release the expired dedupe key"
    );
    ensure!(queue.ack(&replacement).await?);
    Ok(())
}

async fn concurrent_claims_eventually_reserve_each_job_once(queue: &DatabaseQueue) -> Result<()> {
    for index in 0..8 {
        queue
            .enqueue("test_job", json!({"index": index}), None)
            .await?;
    }
    let reserved = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut reserved = Vec::new();
        let mut ids = HashSet::new();
        let mut tokens = HashSet::new();
        let mut indices = HashSet::new();
        // SKIP LOCKED may return None while another transaction holds scanned
        // rows. InnoDB can lock more rows than LIMIT returns when it must sort.
        // Join each wave before retrying so all previous claims have committed;
        // synchronized subsequent waves continue to exercise actual contention.
        for _ in 0..16 {
            let mut tasks = tokio::task::JoinSet::new();
            let start = std::sync::Arc::new(tokio::sync::Barrier::new(16));
            for _ in 0..16 {
                let queue = queue.clone();
                let start = std::sync::Arc::clone(&start);
                tasks.spawn(async move {
                    start.wait().await;
                    queue.claim().await
                });
            }
            while let Some(result) = tasks.join_next().await {
                if let Some(job) = result?? {
                    ensure!(
                        ids.insert(job.job_id),
                        "concurrent workers reserved the same job twice"
                    );
                    ensure!(
                        tokens.insert(job.lock_token),
                        "different reservations shared a lock token"
                    );
                    ensure!(job.attempts == 1, "an unexpired reservation was reclaimed");
                    let index = job.payload["index"]
                        .as_u64()
                        .context("claimed an unrelated job")?;
                    ensure!(
                        indices.insert(index),
                        "the same enqueued payload was reserved twice"
                    );
                    reserved.push(job);
                    ensure!(
                        reserved.len() <= 8,
                        "workers returned more jobs than were enqueued"
                    );
                }
            }
            if reserved.len() == 8 {
                break;
            }
            tokio::task::yield_now().await;
        }
        ensure!(
            reserved.len() == 8,
            "jobs remained unclaimed after bounded concurrent polling"
        );
        ensure!(
            indices == (0_u64..8).collect(),
            "claims did not match the complete enqueued job set"
        );
        ensure!(
            queue.claim().await?.is_none(),
            "queue returned an additional unexpired job"
        );
        Ok::<_, anyhow::Error>(reserved)
    })
    .await
    .context("concurrent claims did not finish before their leases could expire")??;
    for job in reserved {
        ensure!(queue.ack(&job).await?);
    }
    Ok(())
}

async fn expired_and_superseded_claims_cannot_finish(
    queue: &DatabaseQueue,
    database: &DatabasePool,
) -> Result<()> {
    queue.enqueue("test_job", json!({}), None).await?;
    let first = next(queue).await?;
    let forged = ReservedJob {
        lock_token: Uuid::new_v4(),
        ..first.clone()
    };
    ensure!(
        !queue.ack(&forged).await?,
        "wrong owner acknowledged the job"
    );
    ensure!(
        !queue.fail(&forged, "handler_failed").await?,
        "wrong owner failed the job"
    );
    expire_lease(database, first.job_id).await?;
    ensure!(
        !queue.ack(&first).await?,
        "expired owner acknowledged before reclaim"
    );
    ensure!(
        !queue.fail(&first, "handler_failed").await?,
        "expired owner failed before reclaim"
    );
    let second = next(queue).await?;
    ensure!(second.job_id == first.job_id && second.attempts == 2);
    ensure!(
        second.lock_token != first.lock_token,
        "reclaim reused its predecessor's token"
    );
    ensure!(
        !queue.ack(&first).await?,
        "stale owner acknowledged a newer reservation"
    );
    ensure!(
        !queue.fail(&first, "handler_failed").await?,
        "stale owner modified a newer reservation"
    );
    ensure!(queue.ack(&second).await?);
    ensure!(
        !queue.ack(&second).await?,
        "completed reservation acknowledged twice"
    );
    Ok(())
}

async fn retries_wait_and_stop_at_the_attempt_limit(
    queue: &DatabaseQueue,
    database: &DatabasePool,
) -> Result<()> {
    queue.enqueue("test_job", json!({}), None).await?;
    let first = next(queue).await?;
    ensure!(queue.fail(&first, "handler_failed").await?);
    ensure!(
        queue.claim().await?.is_none(),
        "failed job ignored its backoff deadline"
    );
    let state = job_state(database, first.job_id).await?;
    ensure!(state.0 == "pending" && state.1 == 1 && !state.2);
    ensure!(state.3.as_deref() == Some("handler_failed"));

    make_available(database, first.job_id).await?;
    let second = next(queue).await?;
    ensure!(second.attempts == 2);
    ensure!(queue.fail(&second, "handler_failed").await?);
    make_available(database, first.job_id).await?;
    let third = next(queue).await?;
    ensure!(third.attempts == 3);
    ensure!(queue.fail(&third, "handler_failed").await?);
    let state = job_state(database, first.job_id).await?;
    ensure!(
        state.0 == "failed" && state.1 == 3 && !state.2,
        "attempt limit did not become terminal"
    );
    ensure!(queue.claim().await?.is_none(), "terminal job was retried");
    Ok(())
}

async fn final_attempt_expiration_becomes_terminal(database: &DatabasePool) -> Result<()> {
    let queue = DatabaseQueue::new(database.clone(), queue_config(1));
    queue.enqueue("test_job", json!({}), None).await?;
    let last = next(&queue).await?;
    expire_lease(database, last.job_id).await?;
    ensure!(
        queue.claim().await?.is_none(),
        "expired final attempt was reclaimed"
    );
    let state = job_state(database, last.job_id).await?;
    ensure!(
        state.0 == "failed" && state.1 == 1 && !state.2,
        "expired final attempt was stranded"
    );
    ensure!(state.3.as_deref() == Some("attempt_limit_reached"));
    Ok(())
}

async fn maintenance_keeps_recent_and_failed_jobs(
    queue: &DatabaseQueue,
    database: &DatabasePool,
) -> Result<()> {
    queue.enqueue("test_job", json!({}), None).await?;
    let completed = next(queue).await?;
    ensure!(queue.ack(&completed).await?);
    ensure!(
        queue.prune_completed(1).await? == 0,
        "maintenance removed recent completions"
    );
    ensure!(
        queue.prune_completed(0).await.is_err(),
        "zero retention was accepted"
    );
    age_job(database, completed.job_id).await?;
    queue.enqueue("test_job", json!({}), None).await?;
    let pending = next(queue).await?;
    ensure!(queue.fail(&pending, "handler_failed").await?);
    age_job(database, pending.job_id).await?;
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE jobs SET updated_at = clock_timestamp() - INTERVAL '3 days' WHERE status = 'failed'").execute(pool).await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE jobs SET updated_at = UTC_TIMESTAMP(6) - INTERVAL 3 DAY WHERE status = 'failed'").execute(pool).await?;
        }
    }
    ensure!(queue.prune_completed(1).await? == 1);
    ensure!(
        job_state(database, pending.job_id).await?.0 == "pending",
        "maintenance removed an old pending job"
    );
    let failed_count: i64 = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query_scalar(
                "SELECT COUNT(job_id) AS failed_count FROM jobs WHERE status = 'failed'",
            )
            .fetch_one(pool)
            .await?
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query_scalar(
                "SELECT COUNT(job_id) AS failed_count FROM jobs WHERE status = 'failed'",
            )
            .fetch_one(pool)
            .await?
        }
    };
    ensure!(failed_count == 3, "maintenance deleted failed jobs");
    Ok(())
}

async fn permanent_failure_is_fenced_and_sanitized(
    queue: &DatabaseQueue,
    database: &DatabasePool,
) -> Result<()> {
    queue.enqueue("test_job", json!({}), None).await?;
    let job = next(queue).await?;
    let forged = ReservedJob {
        job_id: job.job_id,
        kind: job.kind.clone(),
        payload: job.payload.clone(),
        attempts: job.attempts,
        lock_token: Uuid::new_v4(),
    };
    ensure!(
        !queue
            .fail_permanently(&forged, "invalid_job_payload")
            .await?
    );
    ensure!(
        queue
            .fail_permanently(&job, "raw-secret-must-not-persist")
            .await?
    );
    let state = job_state(database, job.job_id).await?;
    ensure!(state.0 == "failed" && state.1 == 1 && !state.2);
    ensure!(state.3.as_deref() == Some("job_failed"));
    ensure!(!queue.ack(&job).await?, "terminal job accepted stale ACK");
    Ok(())
}

async fn job_state(
    database: &DatabasePool,
    job_id: Uuid,
) -> Result<(String, i64, bool, Option<String>)> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let row = sqlx::query("SELECT status, attempts, locked_until IS NOT NULL AS leased, last_error FROM jobs WHERE job_id = $1")
                .bind(job_id).fetch_one(pool).await?;
            Ok((
                row.try_get("status")?,
                row.try_get("attempts")?,
                row.try_get("leased")?,
                row.try_get("last_error")?,
            ))
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let row = sqlx::query("SELECT status, attempts, locked_until IS NOT NULL AS leased, last_error FROM jobs WHERE job_id = ?")
                .bind(job_id.to_string()).fetch_one(pool).await?;
            Ok((
                String::from_utf8(row.try_get::<Vec<u8>, _>("status")?)?,
                row.try_get("attempts")?,
                row.try_get("leased")?,
                row.try_get::<Option<Vec<u8>>, _>("last_error")?
                    .map(String::from_utf8)
                    .transpose()?,
            ))
        }
    }
}

async fn expire_lease(database: &DatabasePool, job_id: Uuid) -> Result<()> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE jobs SET locked_until = clock_timestamp() - INTERVAL '1 second' WHERE job_id = $1").bind(job_id).execute(pool).await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query(
                "UPDATE jobs SET locked_until = UTC_TIMESTAMP(6) - INTERVAL 1 SECOND WHERE job_id = ?",
            )
            .bind(job_id.to_string())
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

async fn make_available(database: &DatabasePool, job_id: Uuid) -> Result<()> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE jobs SET available_at = clock_timestamp() - INTERVAL '1 second' WHERE job_id = $1").bind(job_id).execute(pool).await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query(
                "UPDATE jobs SET available_at = UTC_TIMESTAMP(6) - INTERVAL 1 SECOND WHERE job_id = ?",
            )
            .bind(job_id.to_string())
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

async fn age_job(database: &DatabasePool, job_id: Uuid) -> Result<()> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query(
                "UPDATE jobs SET updated_at = clock_timestamp() - INTERVAL '3 days' WHERE job_id = $1",
            )
            .bind(job_id)
            .execute(pool)
            .await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query(
                "UPDATE jobs SET updated_at = UTC_TIMESTAMP(6) - INTERVAL 3 DAY WHERE job_id = ?",
            )
            .bind(job_id.to_string())
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

struct ClaimFixture {
    job_id: Uuid,
    status: &'static str,
    available_at: DateTime<Utc>,
    locked_until: Option<DateTime<Utc>>,
    lock_token: Option<Uuid>,
}

async fn claims_follow_eligibility_and_primary_key_ties(
    queue: &DatabaseQueue,
    database: &DatabasePool,
) -> Result<()> {
    let now = Utc::now();
    let created_at = now - chrono::Duration::days(1);
    let due_at = now - chrono::Duration::minutes(2);
    let mut job_ids: Vec<_> = (0..4).map(|_| Uuid::now_v7()).collect();
    job_ids.sort_unstable();
    let fixtures = [
        ClaimFixture {
            job_id: job_ids[0],
            status: "pending",
            available_at: due_at,
            locked_until: None,
            lock_token: None,
        },
        ClaimFixture {
            job_id: job_ids[1],
            status: "pending",
            available_at: due_at,
            locked_until: None,
            lock_token: None,
        },
        ClaimFixture {
            job_id: job_ids[2],
            status: "running",
            // An older enqueue must not jump ahead of an earlier eligible job.
            available_at: created_at,
            locked_until: Some(now - chrono::Duration::minutes(1)),
            lock_token: Some(Uuid::new_v4()),
        },
        ClaimFixture {
            job_id: job_ids[3],
            status: "pending",
            available_at: now + chrono::Duration::hours(1),
            locked_until: None,
            lock_token: None,
        },
    ];
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
                "INSERT INTO jobs (job_id, kind, payload, status, available_at, locked_until, lock_token, created_at) ",
            );
            query.push_values(&fixtures, |mut row, fixture| {
                row.push_bind(fixture.job_id)
                    .push_bind("claim_fixture")
                    .push_bind(sqlx::types::Json(json!({})))
                    .push_bind(fixture.status)
                    .push_bind(fixture.available_at)
                    .push_bind(fixture.locked_until)
                    .push_bind(fixture.lock_token)
                    .push_bind(created_at);
            });
            query.build().execute(pool).await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut query = sqlx::QueryBuilder::<sqlx::MySql>::new(
                "INSERT INTO jobs (job_id, kind, payload, status, available_at, locked_until, lock_token, created_at) ",
            );
            query.push_values(&fixtures, |mut row, fixture| {
                row.push_bind(fixture.job_id.to_string())
                    .push_bind("claim_fixture")
                    .push_bind(sqlx::types::Json(json!({})))
                    .push_bind(fixture.status)
                    .push_bind(fixture.available_at.naive_utc())
                    .push_bind(fixture.locked_until.map(|value| value.naive_utc()))
                    .push_bind(fixture.lock_token.map(|value| value.to_string()))
                    .push_bind(created_at.naive_utc());
            });
            query.build().execute(pool).await?;
        }
    }
    for expected in &job_ids[..3] {
        let job = next(queue).await?;
        ensure!(
            job.job_id == *expected,
            "claim ignored eligibility or UUID tie ordering"
        );
        ensure!(queue.ack(&job).await?);
    }
    ensure!(
        queue.claim().await?.is_none(),
        "future job became eligible early"
    );
    make_available(database, job_ids[3]).await?;
    let job = next(queue).await?;
    ensure!(job.job_id == job_ids[3]);
    ensure!(queue.ack(&job).await?);
    Ok(())
}

async fn maintenance_is_bounded_and_skips_locked_rows(
    queue: &DatabaseQueue,
    database: &DatabasePool,
) -> Result<()> {
    let job_ids: Vec<_> = (0..1_005).map(|_| Uuid::now_v7()).collect();
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
                "INSERT INTO jobs (job_id, kind, payload, status, updated_at) ",
            );
            query.push_values(&job_ids, |mut row, job_id| {
                row.push_bind(*job_id)
                    .push_bind("retention_fixture")
                    .push_bind(sqlx::types::Json(json!({})))
                    .push_bind("completed")
                    .push("statement_timestamp() - INTERVAL '10 days'");
            });
            query.build().execute(pool).await?;
            let mut lock = pool.begin().await?;
            sqlx::query("SELECT job_id FROM jobs WHERE job_id = $1 FOR UPDATE")
                .bind(job_ids[0])
                .fetch_one(&mut *lock)
                .await?;
            assert_retention_batches(queue).await?;
            lock.rollback().await?;
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut query = sqlx::QueryBuilder::<sqlx::MySql>::new(
                "INSERT INTO jobs (job_id, kind, payload, status, updated_at) ",
            );
            query.push_values(&job_ids, |mut row, job_id| {
                row.push_bind(job_id.to_string())
                    .push_bind("retention_fixture")
                    .push_bind(sqlx::types::Json(json!({})))
                    .push_bind("completed")
                    .push("DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 10 DAY)");
            });
            query.build().execute(pool).await?;
            let mut lock = pool.begin().await?;
            sqlx::query("SELECT job_id FROM jobs WHERE job_id = ? FOR UPDATE")
                .bind(job_ids[0].to_string())
                .fetch_one(&mut *lock)
                .await?;
            assert_retention_batches(queue).await?;
            lock.rollback().await?;
        }
    }
    ensure!(
        queue.prune_completed(7).await? == 1,
        "unlocked candidate was lost"
    );
    ensure!(
        queue.prune_completed(7).await? == 0,
        "recent jobs were removed"
    );
    Ok(())
}

async fn assert_retention_batches(queue: &DatabaseQueue) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        ensure!(
            queue.prune_completed(7).await? == 1_000,
            "retention exceeded its batch bound"
        );
        ensure!(
            queue.prune_completed(7).await? == 4,
            "retention did not advance to remaining keys"
        );
        ensure!(
            queue.prune_completed(7).await? == 0,
            "retention removed a locked row"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("retention waited on a locked candidate instead of skipping it")?
}
