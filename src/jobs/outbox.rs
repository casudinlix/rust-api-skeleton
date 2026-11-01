//! SQL delivery ledger for the Redis queue. Network publishing happens only
//! after the short reservation transaction has committed.

use std::time::Duration;

use serde_json::Value;
use sqlx::{Acquire, Database, Row, pool::PoolConnection, types::Json};
use uuid::Uuid;

use crate::{
    config::QueueConfig,
    database::DatabasePool,
    errors::{AppError, AppResult},
};

use super::{
    ReservedJob,
    backoff::retry_delay_with_jitter,
    validation::{safe_failure_reason, validate_job},
};

const MAX_RETIRED_PER_CLAIM: usize = 32;
const PRUNE_BATCH_SIZE: i64 = 1_000;
const EXECUTION_CLAIM_TIMEOUT: Duration = Duration::from_secs(1);

/// A cancelled/uncertain transaction must not put a lock-waiting query back in
/// the shared pool. Successful commits make the connection reusable.
struct ClaimConnection<DB: Database> {
    connection: Option<PoolConnection<DB>>,
    committed: bool,
}

impl<DB: Database> ClaimConnection<DB> {
    fn connection(&mut self) -> AppResult<&mut PoolConnection<DB>> {
        self.connection.as_mut().ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!("execution claim connection is unavailable"))
        })
    }
}

impl<DB: Database> Drop for ClaimConnection<DB> {
    fn drop(&mut self) {
        if !self.committed
            && let Some(connection) = self.connection.take()
        {
            // Graceful TLS shutdown can await the blocked server. Detach and
            // immediately drop the socket instead, releasing the pool permit
            // without leaving its cancelled SELECT to run on a reused session.
            drop(connection.detach());
        }
    }
}

/// Only this application identity crosses the Redis boundary. Payload and kind
/// are read from SQL after acquiring the execution lease.
#[derive(Clone, Copy, Debug)]
pub struct OutboxDelivery {
    pub job_id: Uuid,
    pub relay_token: Uuid,
}

#[derive(Clone)]
pub struct OutboxQueue {
    database: DatabasePool,
    config: QueueConfig,
}

impl OutboxQueue {
    pub fn new(database: DatabasePool, config: QueueConfig) -> Self {
        Self { database, config }
    }

    /// A duplicate business key is a successful no-op, including after success.
    /// For a domain transaction, use the strict transaction helpers below.
    pub async fn enqueue(
        &self,
        kind: &str,
        payload: Value,
        deduplication_key: Option<&str>,
    ) -> AppResult<()> {
        validate_job(kind, &payload, deduplication_key)?;
        let job_id = Uuid::now_v7();
        match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO job_outbox (job_outbox_id, kind, payload, deduplication_key) \
                     VALUES ($1, $2, $3, $4) ON CONFLICT (deduplication_key) DO NOTHING",
                )
                .bind(job_id)
                .bind(kind)
                .bind(Json(payload))
                .bind(deduplication_key)
                .execute(pool)
                .await?;
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                sqlx::query(
                    "INSERT INTO job_outbox (job_outbox_id, kind, payload, deduplication_key) \
                     VALUES (?, ?, ?, ?) ON DUPLICATE KEY UPDATE job_outbox_id = job_outbox_id",
                )
                .bind(job_id.to_string())
                .bind(kind)
                .bind(Json(payload))
                .bind(deduplication_key)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Reserve at most one delivery. A crashed publisher is retried when due;
    /// recovery of the final execution attempt terminalizes the row here.
    pub async fn claim_delivery(&self) -> AppResult<Option<OutboxDelivery>> {
        let lease = self.lease_seconds()?;
        match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for _ in 0..MAX_RETIRED_PER_CLAIM {
                    let row = sqlx::query(
                        "SELECT job_outbox_id, attempts FROM job_outbox \
                         WHERE relay_due_at <= statement_timestamp() \
                         ORDER BY relay_due_at, job_outbox_id LIMIT 1 FOR UPDATE SKIP LOCKED",
                    )
                    .fetch_optional(&mut *tx)
                    .await?;
                    let Some(row) = row else {
                        tx.commit().await?;
                        return Ok(None);
                    };
                    let job_id: Uuid = row.try_get("job_outbox_id")?;
                    let attempts: i64 = row.try_get("attempts")?;
                    if attempts >= i64::from(self.config.max_attempts) {
                        sqlx::query(
                            "UPDATE job_outbox SET status = 'failed', locked_until = NULL, \
                             lock_token = NULL, relay_due_at = NULL, relay_token = NULL, \
                             last_error = 'attempt_limit_reached', updated_at = statement_timestamp() \
                             WHERE job_outbox_id = $1",
                        )
                        .bind(job_id)
                        .execute(&mut *tx)
                        .await?;
                        continue;
                    }
                    let relay_token = Uuid::new_v4();
                    sqlx::query(
                        "UPDATE job_outbox SET relay_token = $2, \
                         relay_due_at = statement_timestamp() + ($3::bigint * INTERVAL '1 second') \
                         WHERE job_outbox_id = $1",
                    )
                    .bind(job_id)
                    .bind(relay_token)
                    .bind(lease)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    return Ok(Some(OutboxDelivery {
                        job_id,
                        relay_token,
                    }));
                }
                tx.commit().await?;
                Ok(None)
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                let mut tx = pool.begin().await?;
                for _ in 0..MAX_RETIRED_PER_CLAIM {
                    let row = sqlx::query(
                        "SELECT job_outbox_id, attempts FROM job_outbox \
                         WHERE relay_due_at <= UTC_TIMESTAMP(6) \
                         ORDER BY relay_due_at, job_outbox_id LIMIT 1 FOR UPDATE SKIP LOCKED",
                    )
                    .fetch_optional(&mut *tx)
                    .await?;
                    let Some(row) = row else {
                        tx.commit().await?;
                        return Ok(None);
                    };
                    let job_id = mysql_uuid(&row, "job_outbox_id")?;
                    let attempts: i64 = row.try_get("attempts")?;
                    if attempts >= i64::from(self.config.max_attempts) {
                        sqlx::query(
                            "UPDATE job_outbox SET status = 'failed', locked_until = NULL, \
                             lock_token = NULL, relay_due_at = NULL, relay_token = NULL, \
                             last_error = 'attempt_limit_reached', updated_at = UTC_TIMESTAMP(6) \
                             WHERE job_outbox_id = ?",
                        )
                        .bind(job_id.to_string())
                        .execute(&mut *tx)
                        .await?;
                        continue;
                    }
                    let relay_token = Uuid::new_v4();
                    sqlx::query(
                        "UPDATE job_outbox SET relay_token = ?, \
                         relay_due_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
                         WHERE job_outbox_id = ?",
                    )
                    .bind(relay_token.to_string())
                    .bind(lease)
                    .bind(job_id.to_string())
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    return Ok(Some(OutboxDelivery {
                        job_id,
                        relay_token,
                    }));
                }
                tx.commit().await?;
                Ok(None)
            }
        }
    }

    /// Publish acknowledgement is not execution acknowledgement. Retain a due
    /// time so an accepted Redis message lost later can be published again.
    pub async fn published(&self, delivery: &OutboxDelivery) -> AppResult<bool> {
        self.finish_delivery(delivery, self.lease_seconds()?).await
    }

    pub async fn publish_failed(&self, delivery: &OutboxDelivery) -> AppResult<bool> {
        self.lease_seconds()?;
        self.finish_delivery(delivery, self.config.retry_base_secs as i64)
            .await
    }

    async fn finish_delivery(&self, delivery: &OutboxDelivery, delay: i64) -> AppResult<bool> {
        let affected = match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => sqlx::query(
                "UPDATE job_outbox SET relay_token = NULL, \
                 relay_due_at = statement_timestamp() + ($3::bigint * INTERVAL '1 second') \
                 WHERE job_outbox_id = $1 AND relay_token = $2 \
                 AND relay_due_at > clock_timestamp() AND status IN ('pending', 'running')",
            )
            .bind(delivery.job_id)
            .bind(delivery.relay_token)
            .bind(delay)
            .execute(pool)
            .await?
            .rows_affected(),
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => sqlx::query(
                "UPDATE job_outbox SET relay_token = NULL, \
                 relay_due_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
                 WHERE job_outbox_id = ? AND relay_token = ? \
                 AND relay_due_at > UTC_TIMESTAMP(6) AND status IN ('pending', 'running')",
            )
            .bind(delay)
            .bind(delivery.job_id.to_string())
            .bind(delivery.relay_token.to_string())
            .execute(pool)
            .await?
            .rows_affected(),
        };
        Ok(affected == 1)
    }

    /// A Redis delivery is a hint to attempt this fenced SQL claim. None means
    /// duplicate, delayed, terminal, unknown, or already owned by another worker.
    pub async fn claim_job(&self, job_id: Uuid) -> AppResult<Option<ReservedJob>> {
        let lease = self.lease_seconds()?;
        if job_id.get_version_num() != 7 {
            return Ok(None);
        }
        tokio::time::timeout(EXECUTION_CLAIM_TIMEOUT, self.claim_execution(job_id, lease))
            .await
            .map_err(|_| AppError::Unavailable)?
    }

    async fn claim_execution(&self, job_id: Uuid, lease: i64) -> AppResult<Option<ReservedJob>> {
        match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                let mut connection = ClaimConnection {
                    connection: Some(pool.acquire().await?),
                    committed: false,
                };
                let mut tx = connection.connection()?.begin().await?;
                let row = sqlx::query(
                    "SELECT job_outbox_id, kind, payload, attempts FROM job_outbox \
                     WHERE job_outbox_id = $1 AND \
                       ((status = 'pending' AND available_at <= clock_timestamp()) \
                        OR (status = 'running' AND locked_until <= clock_timestamp())) \
                     FOR UPDATE",
                )
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    tx.commit().await?;
                    connection.committed = true;
                    return Ok(None);
                };
                let attempts: i64 = row.try_get("attempts")?;
                if attempts >= i64::from(self.config.max_attempts) {
                    sqlx::query(
                        "UPDATE job_outbox SET status = 'failed', locked_until = NULL, \
                         lock_token = NULL, relay_due_at = NULL, relay_token = NULL, \
                         last_error = 'attempt_limit_reached', updated_at = statement_timestamp() \
                         WHERE job_outbox_id = $1",
                    )
                    .bind(job_id)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    connection.committed = true;
                    return Ok(None);
                }
                let kind: String = row.try_get("kind")?;
                let payload: Json<Value> = row.try_get("payload")?;
                let attempts = next_attempt(attempts)?;
                let lock_token = Uuid::new_v4();
                sqlx::query(
                    "UPDATE job_outbox SET status = 'running', attempts = attempts + 1, \
                     lock_token = $2, locked_until = statement_timestamp() + ($3::bigint * INTERVAL '1 second'), \
                     relay_token = NULL, relay_due_at = statement_timestamp() + ($3::bigint * INTERVAL '1 second'), \
                     updated_at = statement_timestamp() WHERE job_outbox_id = $1",
                )
                .bind(job_id)
                .bind(lock_token)
                .bind(lease)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                connection.committed = true;
                Ok(Some(ReservedJob {
                    id: job_id,
                    kind,
                    payload: payload.0,
                    attempts,
                    lock_token,
                }))
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                let mut connection = ClaimConnection {
                    connection: Some(pool.acquire().await?),
                    committed: false,
                };
                let mut tx = connection.connection()?.begin().await?;
                let row = sqlx::query(
                    "SELECT job_outbox_id, kind, payload, attempts FROM job_outbox \
                     WHERE job_outbox_id = ? AND \
                       ((status = 'pending' AND available_at <= UTC_TIMESTAMP(6)) \
                        OR (status = 'running' AND locked_until <= UTC_TIMESTAMP(6))) \
                     FOR UPDATE",
                )
                .bind(job_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    tx.commit().await?;
                    connection.committed = true;
                    return Ok(None);
                };
                let attempts: i64 = row.try_get("attempts")?;
                if attempts >= i64::from(self.config.max_attempts) {
                    sqlx::query(
                        "UPDATE job_outbox SET status = 'failed', locked_until = NULL, \
                         lock_token = NULL, relay_due_at = NULL, relay_token = NULL, \
                         last_error = 'attempt_limit_reached', updated_at = UTC_TIMESTAMP(6) \
                         WHERE job_outbox_id = ?",
                    )
                    .bind(job_id.to_string())
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    connection.committed = true;
                    return Ok(None);
                }
                let kind = crate::database::connection::mysql_text(&row, "kind")?;
                let payload: Json<Value> = row.try_get("payload")?;
                let attempts = next_attempt(attempts)?;
                let lock_token = Uuid::new_v4();
                sqlx::query(
                    "UPDATE job_outbox SET status = 'running', attempts = attempts + 1, \
                     lock_token = ?, locked_until = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND), \
                     relay_token = NULL, relay_due_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND), \
                     updated_at = UTC_TIMESTAMP(6) WHERE job_outbox_id = ?",
                )
                .bind(lock_token.to_string())
                .bind(lease)
                .bind(lease)
                .bind(job_id.to_string())
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                connection.committed = true;
                Ok(Some(ReservedJob {
                    id: job_id,
                    kind,
                    payload: payload.0,
                    attempts,
                    lock_token,
                }))
            }
        }
    }

    pub async fn ack(&self, job: &ReservedJob) -> AppResult<bool> {
        let affected = match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => sqlx::query(
                "UPDATE job_outbox SET status = 'completed', locked_until = NULL, lock_token = NULL, \
                 relay_due_at = NULL, relay_token = NULL, last_error = NULL, updated_at = statement_timestamp() \
                 WHERE job_outbox_id = $1 AND lock_token = $2 AND status = 'running' \
                 AND locked_until > clock_timestamp()",
            )
            .bind(job.id)
            .bind(job.lock_token)
            .execute(pool)
            .await?
            .rows_affected(),
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => sqlx::query(
                "UPDATE job_outbox SET status = 'completed', locked_until = NULL, lock_token = NULL, \
                 relay_due_at = NULL, relay_token = NULL, last_error = NULL, updated_at = UTC_TIMESTAMP(6) \
                 WHERE job_outbox_id = ? AND lock_token = ? AND status = 'running' \
                 AND locked_until > UTC_TIMESTAMP(6)",
            )
            .bind(job.id.to_string())
            .bind(job.lock_token.to_string())
            .execute(pool)
            .await?
            .rows_affected(),
        };
        Ok(affected == 1)
    }

    pub async fn fail(&self, job: &ReservedJob, reason: &str) -> AppResult<bool> {
        self.finish_failed(job, reason, false).await
    }

    pub async fn fail_permanently(&self, job: &ReservedJob, reason: &str) -> AppResult<bool> {
        self.finish_failed(job, reason, true).await
    }

    async fn finish_failed(
        &self,
        job: &ReservedJob,
        reason: &str,
        permanent: bool,
    ) -> AppResult<bool> {
        self.lease_seconds()?;
        let delay =
            retry_delay_with_jitter(self.config.retry_base_secs, job.attempts, job.id) as i64;
        let maximum = if permanent {
            0
        } else {
            i64::from(self.config.max_attempts)
        };
        let reason = safe_failure_reason(reason);
        let affected = match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => sqlx::query(
                "UPDATE job_outbox SET status = CASE WHEN attempts >= $3 THEN 'failed' ELSE 'pending' END, \
                 available_at = CASE WHEN attempts >= $3 THEN available_at \
                    ELSE statement_timestamp() + ($4::bigint * INTERVAL '1 second') END, \
                 relay_due_at = CASE WHEN attempts >= $3 THEN NULL \
                    ELSE statement_timestamp() + ($4::bigint * INTERVAL '1 second') END, \
                 locked_until = NULL, lock_token = NULL, relay_token = NULL, last_error = $5, \
                 updated_at = statement_timestamp() WHERE job_outbox_id = $1 AND lock_token = $2 \
                 AND status = 'running' AND locked_until > clock_timestamp()",
            )
            .bind(job.id)
            .bind(job.lock_token)
            .bind(maximum)
            .bind(delay)
            .bind(reason)
            .execute(pool)
            .await?
            .rows_affected(),
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => sqlx::query(
                "UPDATE job_outbox SET status = CASE WHEN attempts >= ? THEN 'failed' ELSE 'pending' END, \
                 available_at = CASE WHEN attempts >= ? THEN available_at \
                    ELSE DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) END, \
                 relay_due_at = CASE WHEN attempts >= ? THEN NULL \
                    ELSE DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) END, \
                 locked_until = NULL, lock_token = NULL, relay_token = NULL, last_error = ?, \
                 updated_at = UTC_TIMESTAMP(6) WHERE job_outbox_id = ? AND lock_token = ? \
                 AND status = 'running' AND locked_until > UTC_TIMESTAMP(6)",
            )
            .bind(maximum)
            .bind(maximum)
            .bind(delay)
            .bind(maximum)
            .bind(delay)
            .bind(reason)
            .bind(job.id.to_string())
            .bind(job.lock_token.to_string())
            .execute(pool)
            .await?
            .rows_affected(),
        };
        Ok(affected == 1)
    }

    /// One bounded ordered batch. Repeated calls advance by deleting the prior
    /// batch, without OFFSET. Failed and recent completed rows are preserved.
    pub async fn prune_completed(&self, retention_days: u32) -> AppResult<u64> {
        if !(1..=3_650).contains(&retention_days) {
            return Err(AppError::BadRequest(
                "job retention must be 1..3650 days".into(),
            ));
        }
        let days = i64::from(retention_days);
        match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let ids = sqlx::query_scalar::<_, Uuid>(
                    "SELECT job_outbox_id FROM job_outbox WHERE status = 'completed' \
                     AND updated_at < statement_timestamp() - ($1::bigint * INTERVAL '1 day') \
                     ORDER BY updated_at, job_outbox_id LIMIT $2 FOR UPDATE SKIP LOCKED",
                )
                .bind(days)
                .bind(PRUNE_BATCH_SIZE)
                .fetch_all(&mut *tx)
                .await?;
                let affected = if ids.is_empty() {
                    0
                } else {
                    sqlx::query("DELETE FROM job_outbox WHERE job_outbox_id = ANY($1) AND status = 'completed'")
                        .bind(ids)
                        .execute(&mut *tx)
                        .await?
                        .rows_affected()
                };
                tx.commit().await?;
                Ok(affected)
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    "SELECT job_outbox_id FROM job_outbox WHERE status = 'completed' \
                     AND updated_at < DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? DAY) \
                     ORDER BY updated_at, job_outbox_id LIMIT ? FOR UPDATE SKIP LOCKED",
                )
                .bind(days)
                .bind(PRUNE_BATCH_SIZE)
                .fetch_all(&mut *tx)
                .await?;
                let affected = if rows.is_empty() {
                    0
                } else {
                    let mut query = sqlx::QueryBuilder::<sqlx::MySql>::new(
                        "DELETE FROM job_outbox WHERE status = 'completed' AND job_outbox_id IN (",
                    );
                    let mut arguments = query.separated(", ");
                    for row in &rows {
                        arguments.push_bind(mysql_uuid(row, "job_outbox_id")?.to_string());
                    }
                    arguments.push_unseparated(")");
                    query.build().execute(&mut *tx).await?.rows_affected()
                };
                tx.commit().await?;
                Ok(affected)
            }
        }
    }

    fn lease_seconds(&self) -> AppResult<i64> {
        if !(1..=3_600).contains(&self.config.visibility_timeout_secs)
            || !(1..=100).contains(&self.config.max_attempts)
            || !(1..=3_600).contains(&self.config.retry_base_secs)
        {
            return Err(AppError::Internal(anyhow::anyhow!(
                "invalid outbox queue limits"
            )));
        }
        Ok(self.config.visibility_timeout_secs as i64)
    }
}

/// Strict INSERT for a domain write. A dedupe conflict must roll back the
/// surrounding transaction rather than silently commit a domain record alone.
#[cfg(feature = "postgres")]
pub async fn insert_postgres(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: &str,
    payload: &Value,
    deduplication_key: Option<&str>,
) -> AppResult<()> {
    validate_job(kind, payload, deduplication_key)?;
    sqlx::query(
        "INSERT INTO job_outbox (job_outbox_id, kind, payload, deduplication_key) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(kind)
    .bind(Json(payload))
    .bind(deduplication_key)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(feature = "mysql")]
pub async fn insert_mysql(
    transaction: &mut sqlx::Transaction<'_, sqlx::MySql>,
    kind: &str,
    payload: &Value,
    deduplication_key: Option<&str>,
) -> AppResult<()> {
    validate_job(kind, payload, deduplication_key)?;
    sqlx::query(
        "INSERT INTO job_outbox (job_outbox_id, kind, payload, deduplication_key) VALUES (?, ?, ?, ?)",
    )
    .bind(Uuid::now_v7().to_string())
    .bind(kind)
    .bind(Json(payload))
    .bind(deduplication_key)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn next_attempt(attempts: i64) -> AppResult<u32> {
    attempts
        .checked_add(1)
        .and_then(|next| u32::try_from(next).ok())
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("invalid stored outbox attempt count")))
}

#[cfg(feature = "mysql")]
fn mysql_uuid(row: &sqlx::mysql::MySqlRow, column: &str) -> AppResult<Uuid> {
    Uuid::parse_str(&crate::database::connection::mysql_text(row, column)?)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("invalid stored outbox identifier")))
}
