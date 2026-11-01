use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    config::{QueueConfig, QueueDriver},
    database::DatabasePool,
    errors::AppResult,
};

use super::{DatabaseQueue, ReservedJob, outbox::OutboxQueue};

/// Shared dispatch and completion contract. Acquisition belongs to the backend:
/// database workers poll SQL; Redis workers receive an outbox ID from Apalis.
#[async_trait]
pub trait JobQueue: Send + Sync {
    async fn enqueue(
        &self,
        kind: &str,
        payload: Value,
        deduplication_key: Option<&str>,
    ) -> AppResult<()>;
    async fn ack(&self, job: &ReservedJob) -> AppResult<bool>;
    async fn fail(&self, job: &ReservedJob, reason: &str, permanent: bool) -> AppResult<bool>;
    async fn prune_completed(&self, retention_days: u32) -> AppResult<u64>;
}

pub fn queue_backend(database: &DatabasePool, config: &QueueConfig) -> Arc<dyn JobQueue> {
    match config.driver {
        QueueDriver::Database => Arc::new(DatabaseQueue::new(database.clone(), config.clone())),
        QueueDriver::Redis => Arc::new(OutboxQueue::new(database.clone(), config.clone())),
    }
}

#[async_trait]
impl JobQueue for DatabaseQueue {
    async fn enqueue(&self, kind: &str, payload: Value, key: Option<&str>) -> AppResult<()> {
        DatabaseQueue::enqueue(self, kind, payload, key).await
    }

    async fn ack(&self, job: &ReservedJob) -> AppResult<bool> {
        DatabaseQueue::ack(self, job).await
    }

    async fn fail(&self, job: &ReservedJob, reason: &str, permanent: bool) -> AppResult<bool> {
        if permanent {
            self.fail_permanently(job, reason).await
        } else {
            DatabaseQueue::fail(self, job, reason).await
        }
    }

    async fn prune_completed(&self, days: u32) -> AppResult<u64> {
        DatabaseQueue::prune_completed(self, days).await
    }
}

#[async_trait]
impl JobQueue for OutboxQueue {
    async fn enqueue(&self, kind: &str, payload: Value, key: Option<&str>) -> AppResult<()> {
        OutboxQueue::enqueue(self, kind, payload, key).await
    }

    async fn ack(&self, job: &ReservedJob) -> AppResult<bool> {
        OutboxQueue::ack(self, job).await
    }

    async fn fail(&self, job: &ReservedJob, reason: &str, permanent: bool) -> AppResult<bool> {
        if permanent {
            self.fail_permanently(job, reason).await
        } else {
            OutboxQueue::fail(self, job, reason).await
        }
    }

    async fn prune_completed(&self, days: u32) -> AppResult<u64> {
        OutboxQueue::prune_completed(self, days).await
    }
}
