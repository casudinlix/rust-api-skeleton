use std::time::Duration;

use anyhow::ensure;
use tokio_util::sync::CancellationToken;

use crate::{
    config::{MailConfig, QueueConfig},
    database::DatabasePool,
    repositories,
};

use super::{
    DatabaseQueue, maintenance,
    send_welcome_email::{self, WelcomeEmailHandler},
};

/// Only these safe categories may reach queue failure rows and worker logs.
#[derive(Debug, thiserror::Error)]
pub(crate) enum JobError {
    #[error("invalid_job_payload")]
    InvalidPayload,
    #[error("unsupported_job_kind")]
    UnsupportedKind,
    #[error("user_lookup_failed")]
    UserLookup,
    #[error("invalid_recipient")]
    InvalidRecipient,
    #[error("message_build_failed")]
    MessageBuild,
    #[error("smtp_delivery_failed")]
    SmtpDelivery,
    #[error("maintenance_failed")]
    Maintenance,
    #[error("job_timed_out")]
    TimedOut,
}

/// Runs one job at a time per process; scale with additional worker processes.
/// Cancellation stops new reservations and drains the current bounded job.
pub async fn run_worker(
    database: DatabasePool,
    config: QueueConfig,
    mail: MailConfig,
    cancellation: CancellationToken,
) -> anyhow::Result<()> {
    validate_worker_config(&config)?;
    let queue = DatabaseQueue::new(database.clone(), config.clone());
    let repository = repositories::user_repository(&database);
    let welcome = WelcomeEmailHandler::new(&mail)?;
    let poll = Duration::from_millis(config.poll_interval_ms);
    let deadline = Duration::from_secs(config.job_timeout_secs);
    tracing::info!(concurrency = 1, "queue worker started");

    while !cancellation.is_cancelled() {
        let job = match queue.claim().await {
            Ok(Some(job)) => job,
            Ok(None) => {
                wait_for_poll(poll, &cancellation).await;
                continue;
            }
            Err(_) => {
                tracing::error!(
                    error_code = "queue_claim_failed",
                    "queue claim failed; retrying after poll interval"
                );
                wait_for_poll(poll, &cancellation).await;
                continue;
            }
        };
        // Do not race the handler against cancellation: once claimed, it drains
        // until completion or its own timeout, which is shorter than the lease.
        let result = tokio::time::timeout(deadline, async {
            match job.kind.as_str() {
                send_welcome_email::KIND => {
                    welcome
                        .handle(repository.as_ref(), job.id, &job.payload)
                        .await
                }
                maintenance::KIND => maintenance::handle(&queue, &job.payload).await,
                _ => Err(JobError::UnsupportedKind),
            }
        })
        .await
        .unwrap_or(Err(JobError::TimedOut));

        let transition = match result {
            Ok(()) => queue.ack(&job).await,
            Err(error) => {
                tracing::warn!(job_id = %job.id, attempt = job.attempts, error_code = %error, "job attempt failed");
                queue.fail(&job, &error.to_string()).await
            }
        };
        match transition {
            Ok(true) => {
                tracing::debug!(job_id = %job.id, attempt = job.attempts, "job transition committed")
            }
            Ok(false) => {
                tracing::warn!(job_id = %job.id, attempt = job.attempts, "job lease was lost; stale transition rejected")
            }
            Err(_) => {
                tracing::error!(job_id = %job.id, error_code = "queue_transition_failed", "job transition failed; lease recovery will handle the reservation")
            }
        }
    }
    tracing::info!("queue worker stopped after draining current job");
    Ok(())
}

async fn wait_for_poll(duration: Duration, cancellation: &CancellationToken) {
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => {},
        _ = tokio::time::sleep(duration) => {},
    }
}

fn validate_worker_config(config: &QueueConfig) -> anyhow::Result<()> {
    ensure!(
        config.poll_interval_ms > 0,
        "worker poll interval must be positive"
    );
    ensure!(config.max_attempts > 0, "worker attempts must be positive");
    ensure!(
        config.job_timeout_secs > 0 && config.job_timeout_secs < config.visibility_timeout_secs,
        "job timeout must be positive and strictly shorter than the lease"
    );
    ensure!(
        config.visibility_timeout_secs <= 3_600,
        "worker lease must be at most one hour"
    );
    ensure!(config.retry_base_secs > 0, "retry delay must be positive");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_rejects_a_deadline_that_can_outlive_its_lease() {
        for timeout in [0, 60, 61] {
            let config = QueueConfig {
                poll_interval_ms: 100,
                visibility_timeout_secs: 60,
                job_timeout_secs: timeout,
                max_attempts: 3,
                retry_base_secs: 2,
            };
            assert!(validate_worker_config(&config).is_err());
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_an_idle_poll() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        tokio::time::timeout(
            Duration::from_millis(100),
            wait_for_poll(Duration::from_secs(60), &cancellation),
        )
        .await
        .expect("idle worker did not stop promptly");
    }
}
