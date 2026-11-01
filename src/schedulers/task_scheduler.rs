//! Cron triggers enqueue work; execution belongs to the durable queue worker.

use std::sync::Arc;

use anyhow::ensure;
use chrono::Utc;
use serde_json::json;
use tokio_cron_scheduler::{Job, JobScheduler};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    config::SchedulerConfig,
    jobs::{DatabaseQueue, maintenance},
};

use super::deduplication::maintenance_deduplication_key;

pub async fn run_scheduler(
    queue: Arc<DatabaseQueue>,
    config: SchedulerConfig,
    cancellation: CancellationToken,
) -> anyhow::Result<()> {
    ensure!(
        (1..=3_650).contains(&config.retention_days),
        "job retention must be 1..3650 days"
    );
    let tracker = TaskTracker::new();
    let tracking = tracker.clone();
    let stopping = cancellation.clone();
    let retention_days = config.retention_days;
    let mut scheduler = JobScheduler::new().await?;
    let job = Job::new_async_tz(config.maintenance_cron.as_str(), Utc, move |_, _| {
        let queue = Arc::clone(&queue);
        let stopping = stopping.clone();
        Box::pin(tracking.track_future(async move {
            if stopping.is_cancelled() {
                return;
            }
            let key = maintenance_deduplication_key(Utc::now().timestamp());
            // Dropping an in-flight insert may leave its commit outcome unknown.
            // The same minute key makes a retry safe while allowing shutdown to
            // stop waiting for a stalled database connection or query.
            tokio::select! {
                biased;
                _ = stopping.cancelled() => {},
                result = queue.enqueue(
                    maintenance::KIND,
                    json!({"retention_days": retention_days}),
                    Some(&key),
                ) => {
                    if result.is_err() {
                        tracing::error!(
                            error_code = "maintenance_enqueue_failed",
                            "could not enqueue scheduled maintenance"
                        );
                    }
                }
            }
        }))
    })
    .map_err(|_| {
        anyhow::anyhow!("maintenance cron must be a valid expression including seconds")
    })?;
    scheduler.add(job).await?;
    scheduler.start().await?;
    tracing::info!(timezone = "UTC", "scheduler started");
    cancellation.cancelled().await;
    let shutdown = scheduler.shutdown().await;
    tracker.close();
    tracker.wait().await;
    shutdown?;
    tracing::info!("scheduler stopped after draining enqueues");
    Ok(())
}
