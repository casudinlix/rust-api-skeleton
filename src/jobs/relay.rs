use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::{
    config::{QueueConfig, QueueDriver},
    database::DatabasePool,
};

use super::{outbox::OutboxQueue, redis::RedisTransport, worker::wait_for_poll};

/// Publish committed outbox rows. Redis is never contacted while SQL holds a
/// domain transaction; uncertain publish outcomes are retried by stable job ID.
pub async fn run_relay(
    database: DatabasePool,
    config: QueueConfig,
    cancellation: CancellationToken,
) -> anyhow::Result<()> {
    config.validate()?;
    anyhow::ensure!(
        config.driver == QueueDriver::Redis,
        "outbox relay requires QUEUE_DRIVER=redis"
    );
    let outbox = OutboxQueue::new(database, config.clone());
    let poll = Duration::from_millis(config.poll_interval_ms);
    while !cancellation.is_cancelled() {
        let connection = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Ok(()),
            result = RedisTransport::connect(&config.redis) => result,
        };
        match connection {
            Ok(transport) => {
                return run_with_transport(&outbox, &transport, &config, cancellation).await;
            }
            Err(_) => {
                tracing::error!(
                    error_code = "redis_queue_unavailable",
                    "relay waiting for Redis; outbox retained"
                );
                wait_for_poll(poll, &cancellation).await;
            }
        }
    }
    Ok(())
}

pub(super) async fn run_with_transport(
    outbox: &OutboxQueue,
    transport: &RedisTransport,
    config: &QueueConfig,
    cancellation: CancellationToken,
) -> anyhow::Result<()> {
    let poll = Duration::from_millis(config.poll_interval_ms);
    tracing::info!("outbox relay started");
    while !cancellation.is_cancelled() {
        let mut published = 0_u32;
        // Bounded burst protects the async executor and lets shutdown interrupt a
        // busy relay. Each short SQL claim commits before the Redis operation.
        for _ in 0..32 {
            if cancellation.is_cancelled() {
                break;
            }
            let delivery = tokio::select! {
                biased;
                _ = cancellation.cancelled() => break,
                result = outbox.claim_delivery() => result,
            };
            let delivery = match delivery {
                Ok(Some(delivery)) => delivery,
                Ok(None) => break,
                Err(_) => {
                    tracing::error!(
                        error_code = "outbox_relay_claim_failed",
                        "outbox reservation failed"
                    );
                    break;
                }
            };
            let sent = tokio::select! {
                biased;
                _ = cancellation.cancelled() => break,
                result = transport.publish(delivery.job_id) => result,
            };
            let transition = if sent.is_ok() {
                published += 1;
                outbox.published(&delivery).await
            } else {
                tracing::warn!(job_id = %delivery.job_id, error_code = "outbox_publish_failed", "Redis publish failed; retry scheduled");
                outbox.publish_failed(&delivery).await
            };
            match transition {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!(job_id = %delivery.job_id, "relay completion superseded by another owner")
                }
                Err(_) => {
                    tracing::error!(job_id = %delivery.job_id, error_code = "outbox_relay_transition_failed", "uncertain publish outcome retained for recovery")
                }
            }
            if sent.is_err() {
                break;
            }
        }
        if published > 0 {
            tracing::info!(published, "outbox delivery burst finished");
        }
        if published < 32 {
            wait_for_poll(poll, &cancellation).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
    tracing::info!("outbox relay stopped");
    Ok(())
}
