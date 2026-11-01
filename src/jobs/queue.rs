use serde_json::Value;
use sqlx::types::Json;
use uuid::Uuid;

use crate::{
    config::QueueConfig,
    database::DatabasePool,
    errors::{AppError, AppResult},
};

use super::backoff::retry_delay_secs;

/// A committed reservation. Its token is replaced on every claim, including
/// recovery after a crashed worker. Payloads deliberately do not implement Debug.
#[derive(Clone)]
pub struct ReservedJob {
    pub id: Uuid,
    pub kind: String,
    pub payload: Value,
    pub attempts: u32,
    pub lock_token: Uuid,
}

#[derive(Clone)]
pub struct DatabaseQueue {
    database: DatabasePool,
    config: QueueConfig,
}

impl DatabaseQueue {
    pub fn new(database: DatabasePool, config: QueueConfig) -> Self {
        Self { database, config }
    }

    /// A duplicate key is a successful no-op. Completed rows retain the key until
    /// maintenance prunes them; failed rows retain it for operator inspection.
    pub async fn enqueue(
        &self,
        kind: &str,
        payload: Value,
        deduplication_key: Option<&str>,
    ) -> AppResult<()> {
        if kind.is_empty()
            || kind.len() > 64
            || !kind
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(AppError::BadRequest("invalid job kind".into()));
        }
        if deduplication_key.is_some_and(|key| {
            key.is_empty()
                || key.len() > 191
                || !key.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.')
                })
        }) {
            return Err(AppError::BadRequest("invalid job deduplication key".into()));
        }
        let id = Uuid::new_v4();
        match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO jobs (id, kind, payload, deduplication_key) VALUES ($1, $2, $3, $4) \
                     ON CONFLICT (deduplication_key) DO NOTHING",
                )
                .bind(id).bind(kind).bind(Json(payload)).bind(deduplication_key)
                .execute(pool).await?;
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                sqlx::query(
                    "INSERT INTO jobs (id, kind, payload, deduplication_key) VALUES (?, ?, ?, ?) \
                     ON DUPLICATE KEY UPDATE id = id",
                )
                .bind(id.to_string())
                .bind(kind)
                .bind(Json(payload))
                .bind(deduplication_key)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Claims one job in a short transaction. Up to 32 exhausted jobs are retired
    /// per call, so crashed final attempts cannot remain running indefinitely.
    /// `None` means no row was claimable during this call, not that the queue is
    /// empty: other transactions can temporarily lock eligible rows. Poll again.
    pub async fn claim(&self) -> AppResult<Option<ReservedJob>> {
        match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => self.claim_postgres(pool).await,
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => self.claim_mysql(pool).await,
        }
    }

    #[cfg(feature = "postgres")]
    async fn claim_postgres(&self, pool: &sqlx::PgPool) -> AppResult<Option<ReservedJob>> {
        #[derive(sqlx::FromRow)]
        struct JobRow {
            id: Uuid,
            kind: String,
            payload: Json<Value>,
            attempts: i64,
        }

        let lease = lease_seconds(&self.config)?;
        let mut transaction = pool.begin().await?;
        for _ in 0..32 {
            let row = sqlx::query_as::<_, JobRow>(
                "SELECT id, kind, payload, attempts FROM jobs \
                 WHERE (status = 'pending' AND available_at <= clock_timestamp()) \
                    OR (status = 'running' AND locked_until <= clock_timestamp()) \
                 ORDER BY available_at, created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED",
            )
            .fetch_optional(&mut *transaction)
            .await?;
            let Some(row) = row else {
                transaction.commit().await?;
                return Ok(None);
            };
            if row.attempts >= i64::from(self.config.max_attempts) {
                sqlx::query(
                    "UPDATE jobs SET status = 'failed', locked_until = NULL, lock_token = NULL, \
                     last_error = 'attempt_limit_reached', updated_at = clock_timestamp() WHERE id = $1",
                ).bind(row.id).execute(&mut *transaction).await?;
                continue;
            }
            let attempts = next_attempt(row.attempts)?;
            let lock_token = Uuid::new_v4();
            sqlx::query(
                "UPDATE jobs SET status = 'running', attempts = attempts + 1, lock_token = $2, \
                 locked_until = clock_timestamp() + ($3::bigint * INTERVAL '1 second'), \
                 updated_at = clock_timestamp() WHERE id = $1",
            )
            .bind(row.id)
            .bind(lock_token)
            .bind(lease)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Ok(Some(ReservedJob {
                id: row.id,
                kind: row.kind,
                payload: row.payload.0,
                attempts,
                lock_token,
            }));
        }
        transaction.commit().await?;
        Ok(None)
    }

    #[cfg(feature = "mysql")]
    async fn claim_mysql(&self, pool: &sqlx::MySqlPool) -> AppResult<Option<ReservedJob>> {
        #[derive(sqlx::FromRow)]
        struct JobRow {
            // SQLx treats MySQL binary collations as binary wire values even
            // when the column is CHAR/VARCHAR. Decode bytes deliberately.
            id: Vec<u8>,
            kind: Vec<u8>,
            payload: Json<Value>,
            attempts: i64,
        }

        let lease = lease_seconds(&self.config)?;
        let mut transaction = pool.begin().await?;
        for _ in 0..32 {
            let row = sqlx::query_as::<_, JobRow>(
                "SELECT id, kind, payload, attempts FROM jobs \
                 WHERE (status = 'pending' AND available_at <= UTC_TIMESTAMP(6)) \
                    OR (status = 'running' AND locked_until <= UTC_TIMESTAMP(6)) \
                 ORDER BY available_at, created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED",
            )
            .fetch_optional(&mut *transaction)
            .await?;
            let Some(row) = row else {
                transaction.commit().await?;
                return Ok(None);
            };
            let id = std::str::from_utf8(&row.id)
                .ok()
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(|| anyhow::anyhow!("stored job ID is invalid"))?;
            if row.attempts >= i64::from(self.config.max_attempts) {
                sqlx::query(
                    "UPDATE jobs SET status = 'failed', locked_until = NULL, lock_token = NULL, \
                     last_error = 'attempt_limit_reached', updated_at = UTC_TIMESTAMP(6) WHERE id = ?",
                ).bind(id.to_string()).execute(&mut *transaction).await?;
                continue;
            }
            let attempts = next_attempt(row.attempts)?;
            let kind = String::from_utf8(row.kind)
                .map_err(|_| anyhow::anyhow!("stored job kind is not valid UTF-8"))?;
            let lock_token = Uuid::new_v4();
            sqlx::query(
                "UPDATE jobs SET status = 'running', attempts = attempts + 1, lock_token = ?, \
                 locked_until = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND), \
                 updated_at = UTC_TIMESTAMP(6) WHERE id = ?",
            )
            .bind(lock_token.to_string())
            .bind(lease)
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Ok(Some(ReservedJob {
                id,
                kind,
                payload: row.payload.0,
                attempts,
                lock_token,
            }));
        }
        transaction.commit().await?;
        Ok(None)
    }

    /// Returns false when the lease expired or another attempt owns the job.
    pub async fn ack(&self, job: &ReservedJob) -> AppResult<bool> {
        let affected = match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => sqlx::query(
                "UPDATE jobs SET status = 'completed', locked_until = NULL, lock_token = NULL, \
                 last_error = NULL, updated_at = clock_timestamp() \
                 WHERE id = $1 AND lock_token = $2 AND status = 'running' AND locked_until > clock_timestamp()",
            ).bind(job.id).bind(job.lock_token).execute(pool).await?.rows_affected(),
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => sqlx::query(
                "UPDATE jobs SET status = 'completed', locked_until = NULL, lock_token = NULL, \
                 last_error = NULL, updated_at = UTC_TIMESTAMP(6) \
                 WHERE id = ? AND lock_token = ? AND status = 'running' AND locked_until > UTC_TIMESTAMP(6)",
            ).bind(job.id.to_string()).bind(job.lock_token.to_string()).execute(pool).await?.rows_affected(),
        };
        Ok(affected == 1)
    }

    /// Store a safe error category, never a raw external-service error containing
    /// credentials, message bodies, or recipient addresses.
    pub async fn fail(&self, job: &ReservedJob, reason: &str) -> AppResult<bool> {
        let delay = retry_delay_secs(self.config.retry_base_secs, job.attempts) as i64;
        let maximum = i64::from(self.config.max_attempts);
        let reason: String = reason.chars().take(512).collect();
        let affected = match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => sqlx::query(
                "UPDATE jobs SET status = CASE WHEN attempts >= $3 THEN 'failed' ELSE 'pending' END, \
                 available_at = CASE WHEN attempts >= $3 THEN available_at \
                     ELSE clock_timestamp() + ($4::bigint * INTERVAL '1 second') END, \
                 locked_until = NULL, lock_token = NULL, last_error = $5, updated_at = clock_timestamp() \
                 WHERE id = $1 AND lock_token = $2 AND status = 'running' AND locked_until > clock_timestamp()",
            ).bind(job.id).bind(job.lock_token).bind(maximum).bind(delay).bind(&reason)
                .execute(pool).await?.rows_affected(),
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => sqlx::query(
                "UPDATE jobs SET status = CASE WHEN attempts >= ? THEN 'failed' ELSE 'pending' END, \
                 available_at = CASE WHEN attempts >= ? THEN available_at \
                     ELSE DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) END, \
                 locked_until = NULL, lock_token = NULL, last_error = ?, updated_at = UTC_TIMESTAMP(6) \
                 WHERE id = ? AND lock_token = ? AND status = 'running' AND locked_until > UTC_TIMESTAMP(6)",
            ).bind(maximum).bind(maximum).bind(delay).bind(&reason)
                .bind(job.id.to_string()).bind(job.lock_token.to_string())
                .execute(pool).await?.rows_affected(),
        };
        Ok(affected == 1)
    }

    /// Deletes only completed jobs. Deduplication lasts at least until this
    /// retention interval after completion; failed jobs are never pruned here.
    pub async fn prune_completed(&self, retention_days: u32) -> AppResult<u64> {
        if !(1..=3_650).contains(&retention_days) {
            return Err(AppError::BadRequest(
                "job retention must be 1..3650 days".into(),
            ));
        }
        let days = i64::from(retention_days);
        let deleted = match &self.database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => sqlx::query(
                "DELETE FROM jobs WHERE status = 'completed' \
                 AND updated_at < clock_timestamp() - ($1::bigint * INTERVAL '1 day')",
            )
            .bind(days)
            .execute(pool)
            .await?
            .rows_affected(),
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => sqlx::query(
                "DELETE FROM jobs WHERE status = 'completed' \
                 AND updated_at < DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? DAY)",
            )
            .bind(days)
            .execute(pool)
            .await?
            .rows_affected(),
        };
        Ok(deleted)
    }
}

fn lease_seconds(config: &QueueConfig) -> AppResult<i64> {
    i64::try_from(config.visibility_timeout_secs)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("queue lease exceeds supported range")))
}

fn next_attempt(attempts: i64) -> AppResult<u32> {
    attempts
        .checked_add(1)
        .and_then(|next| u32::try_from(next).ok())
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("stored job attempt count is invalid")))
}
