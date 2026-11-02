use std::{sync::Arc, time::Duration};

use anyhow::{Context, ensure};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{
    config::{MailConfig, QueueConfig, QueueDriver},
    database::DatabasePool,
    observability::{JobOutcome, Metrics, ProcessProbe},
    repositories::{self, UserRepository},
};

use super::{
    DatabaseQueue, JobQueue, ReservedJob, maintenance,
    outbox::OutboxQueue,
    queue_backend,
    redis::RedisTransport,
    relay,
    send_welcome_email::{self, WelcomeEmailHandler},
};

/// Only safe categories may enter queue records or structured worker logs.
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

impl JobError {
    fn permanent(&self) -> bool {
        matches!(
            self,
            Self::InvalidPayload
                | Self::UnsupportedKind
                | Self::InvalidRecipient
                | Self::MessageBuild
        )
    }
}

struct JobExecutor {
    queue: Arc<dyn JobQueue>,
    users: Arc<dyn UserRepository>,
    welcome: WelcomeEmailHandler,
    timeout: Duration,
    metrics: Metrics,
}

impl JobExecutor {
    async fn execute(&self, job: ReservedJob) -> anyhow::Result<()> {
        let started = tokio::time::Instant::now();
        self.metrics.job_started();
        tracing::info!(job_id = %job.job_id, attempt = job.attempts, kind = %job.kind, "job execution started");
        // Cancellation stops acquisition, but a claimed job gets its bounded
        // drain. If this future is dropped, its SQL lease remains recoverable.
        let outcome = tokio::time::timeout(self.timeout, async {
            match job.kind.as_str() {
                send_welcome_email::KIND => {
                    self.welcome
                        .handle(self.users.as_ref(), job.job_id, &job.payload)
                        .await
                }
                maintenance::KIND => maintenance::handle(self.queue.as_ref(), &job.payload).await,
                _ => Err(JobError::UnsupportedKind),
            }
        })
        .await
        .unwrap_or(Err(JobError::TimedOut));

        let handler_failed = outcome.is_err();
        let (transition, status) = match outcome {
            Ok(()) => (self.queue.ack(&job).await, "completed"),
            Err(error) => {
                let permanent = error.permanent();
                tracing::warn!(job_id = %job.job_id, attempt = job.attempts, error_code = %error, permanent, "job execution failed");
                (
                    self.queue.fail(&job, &error.to_string(), permanent).await,
                    "failed_or_retry",
                )
            }
        };
        let elapsed = started.elapsed();
        match transition {
            Ok(true) => {
                self.metrics.job_finished(
                    if handler_failed {
                        JobOutcome::HandlerFailed
                    } else {
                        JobOutcome::Completed
                    },
                    elapsed,
                );
                tracing::info!(job_id = %job.job_id, status, elapsed_ms = elapsed.as_millis() as u64, "job transition committed")
            }
            Ok(false) => {
                self.metrics.job_finished(JobOutcome::LeaseLost, elapsed);
                tracing::warn!(job_id = %job.job_id, "job lease lost; stale completion rejected")
            }
            Err(_) => {
                self.metrics
                    .job_finished(JobOutcome::TransitionError, elapsed);
                tracing::error!(job_id = %job.job_id, error_code = "queue_transition_failed", "job remains recoverable after lease expiry");
                anyhow::bail!("queue_transition_failed");
            }
        }
        Ok(())
    }
}

pub async fn run_worker(
    database: DatabasePool,
    config: QueueConfig,
    mail: MailConfig,
    cancellation: CancellationToken,
) -> anyhow::Result<()> {
    run_worker_with_probe(database, config, mail, cancellation, ProcessProbe::new()).await
}

pub async fn run_worker_with_probe(
    database: DatabasePool,
    config: QueueConfig,
    mail: MailConfig,
    cancellation: CancellationToken,
    probe: ProcessProbe,
) -> anyhow::Result<()> {
    validate_worker_config(&config)?;
    let executor = Arc::new(JobExecutor {
        queue: queue_backend(&database, &config),
        users: repositories::user_repository(&database),
        welcome: WelcomeEmailHandler::new(&mail)?,
        timeout: Duration::from_secs(config.job_timeout_secs),
        metrics: probe.metrics(),
    });
    tracing::info!(driver = %config.driver, concurrency = config.concurrency, "queue worker started");
    let result = match config.driver {
        QueueDriver::Database => {
            run_database(database, config, executor, cancellation, probe.clone()).await
        }
        QueueDriver::Redis => {
            run_redis(database, config, executor, cancellation, probe.clone()).await
        }
    };
    probe.set_ready(false);
    result?;
    tracing::info!("queue worker stopped after draining current jobs");
    Ok(())
}

async fn run_database(
    database: DatabasePool,
    config: QueueConfig,
    executor: Arc<JobExecutor>,
    cancellation: CancellationToken,
    probe: ProcessProbe,
) -> anyhow::Result<()> {
    probe.set_ready(true);
    // JoinSet owns every task: dropping the service aborts its acquisition loops
    // instead of leaving detached work past the shutdown deadline.
    let mut workers = JoinSet::new();
    for _ in 0..config.concurrency {
        let queue = DatabaseQueue::new(database.clone(), config.clone());
        let executor = executor.clone();
        let stopping = cancellation.clone();
        let poll = Duration::from_millis(config.poll_interval_ms);
        workers.spawn(async move {
            while !stopping.is_cancelled() {
                let claim = tokio::select! {
                    biased;
                    _ = stopping.cancelled() => break,
                    result = queue.claim() => result,
                };
                match claim {
                    Ok(Some(job)) => {
                        let _ = executor.execute(job).await;
                    }
                    Ok(None) => wait_for_poll(poll, &stopping).await,
                    Err(_) => {
                        executor.metrics.queue_claim_failed();
                        tracing::error!(
                            error_code = "queue_claim_failed",
                            "queue claim failed; retrying after poll interval"
                        );
                        wait_for_poll(poll, &stopping).await;
                    }
                }
            }
        });
    }
    let mut panicked = false;
    while let Some(result) = workers.join_next().await {
        if result.is_err() {
            cancellation.cancel();
            panicked = true;
        }
    }
    ensure!(!panicked, "database worker task stopped unexpectedly");
    Ok(())
}

async fn run_redis(
    database: DatabasePool,
    config: QueueConfig,
    executor: Arc<JobExecutor>,
    cancellation: CancellationToken,
    probe: ProcessProbe,
) -> anyhow::Result<()> {
    let outbox = OutboxQueue::new(database, config.clone());
    let poll = Duration::from_millis(config.poll_interval_ms);
    while !cancellation.is_cancelled() {
        probe.set_ready(false);
        let connection = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Ok(()),
            result = RedisTransport::connect(&config.redis) => result,
        };
        let transport = match connection {
            Ok(transport) => transport,
            Err(_) => {
                executor.metrics.redis_reconnect();
                tracing::error!(
                    error_code = "redis_queue_unavailable",
                    "Redis queue unavailable; outbox remains durable"
                );
                wait_for_poll(poll, &cancellation).await;
                continue;
            }
        };
        probe.set_ready(true);
        let child = cancellation.child_token();
        let consumer = transport.run(child.clone(), config.concurrency, {
            let outbox = outbox.clone();
            let executor = executor.clone();
            move |job_id| {
                let outbox = outbox.clone();
                let executor = executor.clone();
                async move {
                    match outbox.claim_job(job_id).await {
                        Ok(Some(job)) => executor.execute(job).await,
                        // Duplicate, early, terminal or currently owned delivery:
                        // ACK transport only. The durable ledger retains recovery.
                        Ok(None) => Ok(()),
                        Err(_) => anyhow::bail!("outbox_claim_failed"),
                    }
                }
            }
        });
        let relay = async {
            if config.relay_enabled {
                relay::run_with_transport_observed(
                    &outbox,
                    &transport,
                    &config,
                    child.clone(),
                    executor.metrics.clone(),
                )
                .await
            } else {
                child.cancelled().await;
                Ok(())
            }
        };
        tokio::pin!(consumer, relay);
        let result = tokio::select! {
            result = &mut consumer => {
                child.cancel();
                let _ = relay.await;
                result.map_err(|_| anyhow::anyhow!("redis_queue_consumer_failed"))
            },
            result = &mut relay => {
                child.cancel();
                let _ = consumer.await;
                result.context("redis outbox relay stopped")
            },
        };
        probe.set_ready(false);
        if !cancellation.is_cancelled() {
            executor.metrics.redis_reconnect();
            tracing::error!(
                error_code = "redis_queue_restarting",
                failed = result.is_err(),
                "Redis queue session stopped; reconnecting"
            );
            wait_for_poll(poll, &cancellation).await;
        }
    }
    Ok(())
}

pub(super) async fn wait_for_poll(poll: Duration, cancellation: &CancellationToken) {
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => {},
        _ = tokio::time::sleep(poll) => {},
    }
}

fn validate_worker_config(config: &QueueConfig) -> anyhow::Result<()> {
    config.validate()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_rejects_a_deadline_that_can_outlive_its_lease() {
        for timeout in [0, 60, 61] {
            let config = QueueConfig {
                visibility_timeout_secs: 60,
                job_timeout_secs: timeout,
                ..QueueConfig::default()
            };
            assert!(validate_worker_config(&config).is_err());
        }
    }

    #[test]
    fn permanent_payload_errors_do_not_consume_all_retries() {
        assert!(JobError::InvalidPayload.permanent());
        assert!(JobError::UnsupportedKind.permanent());
        assert!(!JobError::SmtpDelivery.permanent());
        assert!(!JobError::UserLookup.permanent());
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
