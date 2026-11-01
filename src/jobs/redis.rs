//! Redis transports small wake-up messages; the SQL outbox owns job execution.
//!
//! Apalis transport IDs are internal ULIDs. Application job IDs remain UUIDv7,
//! and each publish may create another delivery of the same application job.

use std::{fmt, future::Future, sync::Arc, time::Duration};

use apalis::prelude::{
    BackendExt, Codec, Event, EventListenerExt, TaskSink, WorkerBuilder, WorkerBuilderExt,
    WorkerContext, WorkerError,
};
use apalis_redis::{Config as ApalisConfig, RedisStorage};
use redis::{
    Cmd, Pipeline, RedisError, RedisFuture, Value,
    aio::{ConnectionLike, ConnectionManager, ConnectionManagerConfig},
};
use serde::{Deserialize, Serialize};
use tokio::{sync::Notify, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::RedisQueueConfig;

const ENVELOPE_VERSION: u8 = 1;
const MAX_ENVELOPE_BYTES: usize = 128;
const MAX_CONCURRENCY: usize = 64;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(1);
const CLEANUP_BATCH_PER_SET: usize = 64;
const WORKER_METADATA_TTL_SECS: u64 = 300;

// Apalis 1.0's vacuum scans the whole payload hash and retains terminal set
// members. Consume bounded terminal sets instead. Selection and deletion are
// atomic; pending, scheduled and still-owned tasks are never removed.
const PRUNE_TERMINAL: &str = r#"
local removed = 0
for _, terminal_set in ipairs({KEYS[3], KEYS[4]}) do
    local ids = redis.call('SRANDMEMBER', terminal_set, tonumber(ARGV[1]))
    for _, id in ipairs(ids) do
        local metadata = KEYS[2] .. ':' .. id
        local status = redis.call('HGET', metadata, 'status')
        local owner = redis.call('HGET', metadata, 'locked_by')
        local held = owner and redis.call('SISMEMBER', KEYS[6] .. ':' .. owner, id) == 1
        local scheduled = redis.call('ZSCORE', KEYS[5], id)
        if (status == 'Done' or status == 'Killed' or status == 'Failed') and not held and not scheduled then
            redis.call('HDEL', KEYS[1], id)
            redis.call('DEL', metadata)
            redis.call('SREM', KEYS[3], id)
            redis.call('SREM', KEYS[4], id)
            redis.call('SREM', KEYS[7], id)
            removed = removed + 1
        elseif not status then
            redis.call('SREM', terminal_set, id)
        end
    end
end
redis.call('EXPIRE', KEYS[8], tonumber(ARGV[2]))
return removed
"#;

// A successfully drained worker has no reservations. Remove only this process's
// registration. Interrupted workers remain discoverable for orphan recovery.
const CLEAN_WORKER: &str = r#"
if redis.call('SCARD', KEYS[1]) ~= 0 then return 0 end
redis.call('ZREM', KEYS[2], ARGV[1])
redis.call('DEL', KEYS[3])
return 1
"#;

/// Safe categories only: never retain a third-party error, URL or task payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RedisTransportError {
    #[error("invalid Redis queue configuration")]
    Configuration,
    #[error("invalid Redis queue envelope")]
    InvalidEnvelope,
    #[error("Redis queue concurrency must be between 1 and 64")]
    InvalidConcurrency,
    #[error("Redis queue operation timed out")]
    Timeout,
    #[error("Redis queue is unavailable")]
    Unavailable,
    #[error("job execution could not record a durable outcome")]
    HandlerFailed,
}

/// A reusable queue connection whose debug output omits connection settings.
#[derive(Clone)]
pub struct RedisTransport {
    connection: SafeConnection,
    config: ApalisConfig,
    timeout: Duration,
}

impl fmt::Debug for RedisTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedisTransport")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl RedisTransport {
    pub async fn connect(config: &RedisQueueConfig) -> Result<Self, RedisTransportError> {
        config
            .validate()
            .map_err(|_| RedisTransportError::Configuration)?;
        let timeout = Duration::from_millis(config.timeout_ms);
        let client = redis::Client::open(config.url.as_str())
            .map_err(|_| RedisTransportError::Configuration)?;
        let connection_config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(timeout))
            .set_response_timeout(Some(timeout))
            .set_number_of_retries(1)
            .set_min_delay(Duration::from_millis(25))
            .set_max_delay(Duration::from_millis(100))
            .set_pipeline_buffer_size(128)
            .set_concurrency_limit(128);
        let connection = tokio::time::timeout(
            timeout,
            ConnectionManager::new_with_config(client, connection_config),
        )
        .await
        .map_err(|_| RedisTransportError::Timeout)?
        .map_err(transport_error)?;
        let config = ApalisConfig::default()
            .queue(&config.prefix)
            .batch_size(1)
            .heartbeat_interval(Duration::from_secs(10))
            .missed_heartbeats(3)
            .lock_tasks(true)
            // In Apalis 1.0 this also controls completion ACK, not only results.
            .persist_results(true)
            .emit_events(false);
        Ok(Self {
            connection: SafeConnection(connection),
            config,
            timeout,
        })
    }

    /// A timeout has an unknown publish outcome. Retrying with the same job ID
    /// is safe because the SQL ledger fences execution and owns terminal state.
    pub async fn publish(&self, job_id: Uuid) -> Result<(), RedisTransportError> {
        if !is_job_id(job_id) {
            return Err(RedisTransportError::InvalidEnvelope);
        }
        let mut storage = RedisStorage::<Delivery, _>::new(self.connection.clone())
            .with_config(self.config.clone())
            .with_codec(EnvelopeCodec);
        tokio::time::timeout(self.timeout, storage.push(Delivery::Job(job_id)))
            .await
            .map_err(|_| RedisTransportError::Timeout)?
            .map_err(|_| RedisTransportError::Unavailable)
    }

    /// Consume messages with bounded in-flight work and drain on cancellation.
    ///
    /// The handler must enforce its job deadline and commit its SQL ACK/retry
    /// before returning `Ok(())`. A successful no-op for a duplicate is valid.
    /// No handler or cleanup task is detached from this future.
    pub async fn run<F, Fut>(
        &self,
        cancellation: CancellationToken,
        concurrency: usize,
        handler: F,
    ) -> Result<(), RedisTransportError>
    where
        F: Fn(Uuid) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        if !(1..=MAX_CONCURRENCY).contains(&concurrency) {
            return Err(RedisTransportError::InvalidConcurrency);
        }
        if cancellation.is_cancelled() {
            return Ok(());
        }
        let context = WorkerContext::new(&format!("rust-api-{}", Uuid::now_v7()));
        let cleanup_needed = Arc::new(Notify::new());
        let notify = Arc::clone(&cleanup_needed);
        let storage = RedisStorage::<Delivery, _>::new(self.connection.clone())
            .with_config(self.config.clone().batch_size(concurrency))
            .with_codec(EnvelopeCodec)
            .poll_with_interval(POLL_INTERVAL);
        let worker = WorkerBuilder::new(&context)
            .backend(storage)
            .concurrency(concurrency)
            .on_event(move |_, event| match event {
                Event::Start => tracing::info!(concurrency, "Redis queue consumer started"),
                Event::Error(_) => {
                    tracing::warn!(
                        error_code = "redis_job_handler_failed",
                        "Redis delivery failed"
                    );
                    notify.notify_one();
                }
                Event::Success(_) => notify.notify_one(),
                _ => {}
            })
            .build(move |delivery: Delivery| {
                let handler = handler.clone();
                async move {
                    match delivery {
                        Delivery::Job(job_id) => handler(job_id)
                            .await
                            .map_err(|_| RedisTransportError::HandlerFailed),
                        Delivery::Discard => {
                            tracing::warn!(
                                error_code = "invalid_redis_job_envelope",
                                "discarding invalid Redis delivery"
                            );
                            // ACK poison envelopes without dispatching domain work.
                            Ok(())
                        }
                    }
                }
            });
        let stopping = cancellation.clone();
        let worker_future = worker.run_until(async move {
            stopping.cancelled().await;
            Ok::<(), WorkerError>(())
        });
        let maintenance = self.maintain(&context, cleanup_needed, cancellation);
        tokio::pin!(worker_future, maintenance);
        let result = tokio::select! {
            result = &mut worker_future => result,
            () = &mut maintenance => worker_future.await,
        };
        // Completion ACKs are flushed by Apalis during the drain. A final bounded
        // pass also keeps short-lived worker invocations from retaining all results.
        if self.prune_terminal(&context).await.is_err() {
            tracing::warn!(
                error_code = "redis_queue_cleanup_failed",
                "Redis queue cleanup deferred"
            );
        }
        if result.is_ok() && self.clean_worker(&context).await.is_err() {
            tracing::warn!(
                error_code = "redis_worker_cleanup_failed",
                "Redis worker cleanup deferred"
            );
        }
        result.map_err(|_| RedisTransportError::Unavailable)?;
        tracing::info!("Redis queue consumer stopped after draining current jobs");
        Ok(())
    }

    async fn maintain(
        &self,
        worker: &WorkerContext,
        notify: Arc<Notify>,
        cancellation: CancellationToken,
    ) {
        let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return,
                _ = interval.tick() => {},
                _ = notify.notified() => {},
            }
            match self.prune_terminal(worker).await {
                Ok(removed) if removed >= CLEANUP_BATCH_PER_SET => notify.notify_one(),
                Ok(_) => {}
                Err(_) => tracing::warn!(
                    error_code = "redis_queue_cleanup_failed",
                    "Redis queue cleanup deferred"
                ),
            }
        }
    }

    async fn prune_terminal(&self, worker: &WorkerContext) -> Result<usize, RedisTransportError> {
        let mut connection = self.connection.clone();
        let config = &self.config;
        let script = redis::Script::new(PRUNE_TERMINAL);
        let mut invocation = script.prepare_invoke();
        invocation
            .key(config.job_data_hash())
            .key(config.job_meta_hash())
            .key(config.done_jobs_set())
            .key(config.dead_jobs_set())
            .key(config.scheduled_jobs_set())
            .key(config.inflight_jobs_set())
            .key(config.failed_jobs_set())
            .key(config.worker_metadata_key(worker))
            .arg(CLEANUP_BATCH_PER_SET)
            .arg(WORKER_METADATA_TTL_SECS);
        tokio::time::timeout(self.timeout, invocation.invoke_async(&mut connection))
            .await
            .map_err(|_| RedisTransportError::Timeout)?
            .map_err(transport_error)
    }

    async fn clean_worker(&self, worker: &WorkerContext) -> Result<(), RedisTransportError> {
        let mut connection = self.connection.clone();
        let script = redis::Script::new(CLEAN_WORKER);
        let mut invocation = script.prepare_invoke();
        invocation
            .key(self.config.inflight_set_for(worker))
            .key(self.config.workers_set())
            .key(self.config.worker_metadata_key(worker))
            .arg(worker.name());
        tokio::time::timeout(
            self.timeout,
            invocation.invoke_async::<u32>(&mut connection),
        )
        .await
        .map_err(|_| RedisTransportError::Timeout)?
        .map_err(transport_error)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Delivery {
    Job(Uuid),
    Discard,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema_version: u8,
    job_id: Uuid,
}

#[derive(Clone, Copy, Debug)]
struct EnvelopeCodec;

impl Codec<Delivery> for EnvelopeCodec {
    type Error = RedisTransportError;
    type Compact = Vec<u8>;

    fn encode(&self, value: &Delivery) -> Result<Vec<u8>, Self::Error> {
        let Delivery::Job(job_id) = value else {
            return Err(RedisTransportError::InvalidEnvelope);
        };
        if !is_job_id(*job_id) {
            return Err(RedisTransportError::InvalidEnvelope);
        }
        let bytes = serde_json::to_vec(&Envelope {
            schema_version: ENVELOPE_VERSION,
            job_id: *job_id,
        })
        .map_err(|_| RedisTransportError::InvalidEnvelope)?;
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(RedisTransportError::InvalidEnvelope);
        }
        Ok(bytes)
    }

    fn decode(&self, bytes: &Vec<u8>) -> Result<Delivery, Self::Error> {
        // Never parse unbounded input or expose a serde error containing input.
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Ok(Delivery::Discard);
        }
        match serde_json::from_slice::<Envelope>(bytes) {
            Ok(envelope)
                if envelope.schema_version == ENVELOPE_VERSION && is_job_id(envelope.job_id) =>
            {
                Ok(Delivery::Job(envelope.job_id))
            }
            _ => Ok(Delivery::Discard),
        }
    }
}

fn is_job_id(job_id: Uuid) -> bool {
    job_id.get_version_num() == 7 && job_id.get_variant() == uuid::Variant::RFC4122
}

/// Apalis may format database errors internally. Strip server text at the
/// connection boundary while keeping NOSCRIPT intact for its script loader.
#[derive(Clone)]
struct SafeConnection(ConnectionManager);

impl fmt::Debug for SafeConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SafeConnection")
    }
}

impl ConnectionLike for SafeConnection {
    fn req_packed_command<'a>(&'a mut self, command: &'a Cmd) -> RedisFuture<'a, Value> {
        Box::pin(async move { self.0.req_packed_command(command).await.map_err(safe_error) })
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        pipeline: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        Box::pin(async move {
            self.0
                .req_packed_commands(pipeline, offset, count)
                .await
                .map_err(safe_error)
        })
    }

    fn get_db(&self) -> i64 {
        self.0.get_db()
    }
}

fn safe_error(error: RedisError) -> RedisError {
    if error.is_timeout() {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Redis queue operation timed out",
        )
        .into()
    } else {
        RedisError::from((error.kind(), "Redis queue operation failed"))
    }
}

fn transport_error(error: RedisError) -> RedisTransportError {
    if error.is_timeout() {
        RedisTransportError::Timeout
    } else {
        RedisTransportError::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::{Semaphore, mpsc};

    use super::*;

    #[test]
    fn redis_envelope_is_small_versioned_and_preserves_application_uuidv7() {
        let job_id = Uuid::now_v7();
        let encoded = EnvelopeCodec.encode(&Delivery::Job(job_id)).unwrap();
        assert!(encoded.len() <= MAX_ENVELOPE_BYTES);
        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["job_id"], job_id.to_string());
        assert_eq!(
            EnvelopeCodec.decode(&encoded).unwrap(),
            Delivery::Job(job_id)
        );
    }

    #[test]
    fn redis_poison_envelopes_are_discarded_without_retaining_rejected_values() {
        let job_id = Uuid::now_v7();
        let mut invalid_variant = *job_id.as_bytes();
        invalid_variant[8] &= 0x7f;
        let candidates = [
            b"not-json-sensitive-token".to_vec(),
            vec![b'a'; MAX_ENVELOPE_BYTES + 1],
            serde_json::to_vec(&serde_json::json!({"schema_version": 2, "job_id": job_id})).unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema_version": 1, "job_id": Uuid::new_v4()})).unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema_version": 1, "job_id": Uuid::nil()})).unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema_version": 1, "job_id": Uuid::from_bytes(invalid_variant)})).unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema_version": 1, "job_id": job_id, "secret": "sensitive-token"})).unwrap(),
        ];
        for candidate in candidates {
            let result = EnvelopeCodec.decode(&candidate).unwrap();
            assert_eq!(result, Delivery::Discard);
            assert!(!format!("{result:?}").contains("sensitive-token"));
        }
        assert_eq!(
            EnvelopeCodec.encode(&Delivery::Job(Uuid::new_v4())),
            Err(RedisTransportError::InvalidEnvelope)
        );
    }

    #[test]
    fn redis_error_sanitization_preserves_script_cache_recovery() {
        let kind = redis::ErrorKind::Server(redis::ServerErrorKind::NoScript);
        let original = RedisError::from((kind, "sensitive-token"));
        let sanitized = safe_error(original);
        assert_eq!(sanitized.kind(), kind);
        assert!(!format!("{sanitized:?}").contains("sensitive-token"));
        assert!(!sanitized.to_string().contains("sensitive-token"));
        let timeout: RedisError =
            std::io::Error::new(std::io::ErrorKind::TimedOut, "sensitive-token").into();
        let sanitized = safe_error(timeout);
        assert!(sanitized.is_timeout());
        assert_eq!(transport_error(sanitized), RedisTransportError::Timeout);
    }

    async fn test_transport() -> RedisTransport {
        let config = RedisQueueConfig {
            url: std::env::var("TEST_REDIS_URL")
                .expect("TEST_REDIS_URL must point to a disposable Redis"),
            prefix: format!("rust-api-transport-test:{}", Uuid::now_v7()),
            timeout_ms: 1_000,
        };
        RedisTransport::connect(&config).await.unwrap()
    }

    // Only this test's finite fixture is inspected. Production cleanup uses the
    // bounded Lua script above, never KEYS, SCAN or a whole payload-hash scan.
    async fn remove_test_keys(transport: &RedisTransport) {
        let config = &transport.config;
        let mut connection = transport.connection.clone();
        let ids: Vec<String> = redis::cmd("HKEYS")
            .arg(config.job_data_hash())
            .query_async(&mut connection)
            .await
            .unwrap();
        let workers: Vec<String> = redis::cmd("ZRANGE")
            .arg(config.workers_set())
            .arg(0)
            .arg(-1)
            .query_async(&mut connection)
            .await
            .unwrap();
        let mut keys = vec![
            config.job_data_hash(),
            config.active_jobs_list(),
            config.workers_set(),
            config.done_jobs_set(),
            config.dead_jobs_set(),
            config.failed_jobs_set(),
            config.scheduled_jobs_set(),
            config.signal_list(),
        ];
        keys.extend(
            ids.into_iter()
                .map(|id| format!("{}:{id}", config.job_meta_hash())),
        );
        for worker in workers {
            let context = WorkerContext::new(&worker);
            keys.push(config.inflight_set_for(&context));
            keys.push(config.worker_metadata_key(&context));
        }
        redis::cmd("DEL")
            .arg(keys)
            .query_async::<u32>(&mut connection)
            .await
            .unwrap();
        redis::cmd("ZREM")
            .arg("core:apalis:queues:list")
            .arg(config.workers_set())
            .query_async::<u32>(&mut connection)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
    async fn redis_transports_duplicate_ids_discards_poison_and_bounds_concurrency() {
        let transport = test_transport().await;
        let duplicate = Uuid::now_v7();
        let jobs = [
            duplicate,
            duplicate,
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        ];
        for job_id in jobs {
            transport.publish(job_id).await.unwrap();
        }
        let mut raw = transport.connection.clone();
        let broker_ids: Vec<String> = redis::cmd("HKEYS")
            .arg(transport.config.job_data_hash())
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(broker_ids.len(), jobs.len());
        assert_ne!(broker_ids[0], broker_ids[1]);
        assert!(broker_ids.iter().all(|id| id.len() == 26));
        let bytes: Vec<Vec<u8>> = redis::cmd("HMGET")
            .arg(transport.config.job_data_hash())
            .arg(&broker_ids)
            .query_async(&mut raw)
            .await
            .unwrap();
        let mut encoded_jobs: Vec<_> = bytes
            .iter()
            .map(|bytes| {
                let Delivery::Job(job_id) = EnvelopeCodec.decode(bytes).unwrap() else {
                    panic!("publisher encoded an invalid envelope");
                };
                job_id
            })
            .collect();
        encoded_jobs.sort_unstable();
        let mut expected = jobs.to_vec();
        expected.sort_unstable();
        assert_eq!(encoded_jobs, expected);
        transport.publish(Uuid::now_v7()).await.unwrap();
        let after_publish: Vec<String> = redis::cmd("HKEYS")
            .arg(transport.config.job_data_hash())
            .query_async(&mut raw)
            .await
            .unwrap();
        let poison_id = after_publish
            .iter()
            .find(|id| !broker_ids.contains(id))
            .unwrap();
        redis::cmd("HSET")
            .arg(transport.config.job_data_hash())
            .arg(poison_id)
            .arg(vec![b'x'; MAX_ENVELOPE_BYTES + 1])
            .query_async::<u32>(&mut raw)
            .await
            .unwrap();

        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let (finished, mut completed) = mpsc::channel(8);
        let stopping = CancellationToken::new();
        let worker = tokio::spawn({
            let transport = transport.clone();
            let stopping = stopping.clone();
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            async move {
                transport
                    .run(stopping, 2, move |job_id| {
                        let active = Arc::clone(&active);
                        let maximum = Arc::clone(&maximum);
                        let finished = finished.clone();
                        async move {
                            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                            maximum.fetch_max(count, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            active.fetch_sub(1, Ordering::SeqCst);
                            finished.send(job_id).await.unwrap();
                            Ok(())
                        }
                    })
                    .await
            }
        });
        let mut actual = Vec::new();
        for _ in jobs {
            actual.push(
                tokio::time::timeout(Duration::from_secs(5), completed.recv())
                    .await
                    .expect("Redis handler was not polled")
                    .unwrap(),
            );
        }
        // Wait for the poison ACK and bounded terminal cleanup, not just callback
        // completion: invalid envelopes must not strand broker reservations.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let remaining: u32 = redis::cmd("HLEN")
                    .arg(transport.config.job_data_hash())
                    .query_async(&mut raw)
                    .await
                    .unwrap();
                if remaining == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("completed transport metadata was retained");
        stopping.cancel();
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(maximum.load(Ordering::SeqCst), 2);
        assert_eq!(completed.recv().await, None);
        remove_test_keys(&transport).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
    async fn redis_cancellation_drains_only_current_work() {
        let transport = test_transport().await;
        for _ in 0..3 {
            transport.publish(Uuid::now_v7()).await.unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Semaphore::new(0));
        let (started, mut receiving) = mpsc::channel(3);
        let stopping = CancellationToken::new();
        let mut worker = tokio::spawn({
            let transport = transport.clone();
            let stopping = stopping.clone();
            let calls = Arc::clone(&calls);
            let release = Arc::clone(&release);
            async move {
                transport
                    .run(stopping, 1, move |_| {
                        let calls = Arc::clone(&calls);
                        let release = Arc::clone(&release);
                        let started = started.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            started.send(()).await.unwrap();
                            release.acquire().await.unwrap().forget();
                            Ok(())
                        }
                    })
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), receiving.recv())
            .await
            .unwrap()
            .unwrap();
        stopping.cancel();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut worker)
                .await
                .is_err(),
            "cancellation dropped a running handler instead of draining"
        );
        release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let mut raw = transport.connection.clone();
        let (pending, scheduled): (u32, u32) = redis::pipe()
            .cmd("LLEN")
            .arg(transport.config.active_jobs_list())
            .cmd("ZCARD")
            .arg(transport.config.scheduled_jobs_set())
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(pending + scheduled, 2);
        let registrations: u32 = redis::cmd("ZCARD")
            .arg(transport.config.workers_set())
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(registrations, 0);
        remove_test_keys(&transport).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
    async fn redis_terminal_cleanup_is_bounded_and_preserves_owned_or_scheduled_tasks() {
        let transport = test_transport().await;
        let config = &transport.config;
        let context = WorkerContext::new("test-cleanup");
        let mut raw = transport.connection.clone();
        let mut fixture = redis::pipe();
        for index in 0..384 {
            let id = format!("terminal-{index}");
            let (status, set) = if index < 256 {
                ("Done", config.done_jobs_set())
            } else {
                ("Failed", config.dead_jobs_set())
            };
            fixture
                .cmd("HSET")
                .arg(config.job_data_hash())
                .arg(&id)
                .arg("payload")
                .ignore()
                .cmd("HSET")
                .arg(format!("{}:{id}", config.job_meta_hash()))
                .arg("status")
                .arg(status)
                .ignore()
                .cmd("SADD")
                .arg(set)
                .arg(&id)
                .ignore();
        }
        fixture.query_async::<()>(&mut raw).await.unwrap();
        assert_eq!(
            transport.prune_terminal(&context).await.unwrap(),
            CLEANUP_BATCH_PER_SET * 2
        );

        for (id, status) in [
            ("owned", "Done"),
            ("scheduled", "Done"),
            ("pending", "Pending"),
        ] {
            redis::pipe()
                .cmd("HSET")
                .arg(config.job_data_hash())
                .arg(id)
                .arg("payload")
                .ignore()
                .cmd("HSET")
                .arg(format!("{}:{id}", config.job_meta_hash()))
                .arg("status")
                .arg(status)
                .arg("locked_by")
                .arg(context.name())
                .ignore()
                .cmd("SADD")
                .arg(config.done_jobs_set())
                .arg(id)
                .ignore()
                .query_async::<()>(&mut raw)
                .await
                .unwrap();
        }
        let outside_key = format!("rust-api-other-test:{}", Uuid::now_v7());
        redis::pipe()
            .cmd("SADD")
            .arg(config.inflight_set_for(&context))
            .arg("owned")
            .ignore()
            .cmd("ZADD")
            .arg(config.scheduled_jobs_set())
            .arg(1)
            .arg("scheduled")
            .ignore()
            .cmd("HSET")
            .arg(config.worker_metadata_key(&context))
            .arg("service")
            .arg("test")
            .ignore()
            .cmd("SET")
            .arg(&outside_key)
            .arg("untouched")
            .arg("EX")
            .arg(30)
            .ignore()
            .query_async::<()>(&mut raw)
            .await
            .unwrap();
        let mut total_removed = CLEANUP_BATCH_PER_SET * 2;
        for _ in 0..8 {
            let removed = transport.prune_terminal(&context).await.unwrap();
            assert!(removed <= CLEANUP_BATCH_PER_SET * 2);
            total_removed += removed;
        }
        assert_eq!(total_removed, 384);
        let remaining: Vec<String> = redis::cmd("HKEYS")
            .arg(config.job_data_hash())
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 3);
        for id in ["owned", "scheduled", "pending"] {
            assert!(remaining.iter().any(|found| found == id));
        }
        let ttl: i64 = redis::cmd("TTL")
            .arg(config.worker_metadata_key(&context))
            .query_async(&mut raw)
            .await
            .unwrap();
        assert!((1..=WORKER_METADATA_TTL_SECS as i64).contains(&ttl));
        let outside: String = redis::cmd("GET")
            .arg(&outside_key)
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(outside, "untouched");
        redis::cmd("DEL")
            .arg(&outside_key)
            .arg(config.inflight_set_for(&context))
            .arg(config.worker_metadata_key(&context))
            .query_async::<u32>(&mut raw)
            .await
            .unwrap();
        remove_test_keys(&transport).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
    async fn redis_authentication_failure_is_bounded_and_sanitized() {
        let mut url = url::Url::parse(&std::env::var("TEST_REDIS_URL").unwrap()).unwrap();
        url.set_username("nonexistent-transport-test-user").unwrap();
        url.set_password(Some("secret-value-must-never-leak"))
            .unwrap();
        let config = RedisQueueConfig {
            url: url.to_string(),
            timeout_ms: 100,
            ..RedisQueueConfig::default()
        };
        let error = tokio::time::timeout(Duration::from_secs(1), RedisTransport::connect(&config))
            .await
            .expect("Redis connect exceeded its deadline")
            .unwrap_err();
        assert!(matches!(
            error,
            RedisTransportError::Timeout | RedisTransportError::Unavailable
        ));
        let rendered = format!("{error:?}: {error}");
        assert!(!rendered.contains("secret-value"));
        assert!(!rendered.contains("nonexistent-transport-test-user"));
        assert!(!rendered.contains("127.0.0.1"));
    }

    #[tokio::test]
    #[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
    async fn redis_large_due_backlog_is_processed_in_bounded_batches() {
        // The unpatched 1.0.0 fetch script attempts unpack(all_due_ids), which
        // exceeds Redis Lua's stack on this backlog before yielding any job.
        const TOTAL: usize = 10_240;
        let transport = test_transport().await;
        let job_id = Uuid::now_v7();
        let mut publisher = RedisStorage::<Delivery, _>::new(transport.connection.clone())
            .with_config(transport.config.clone())
            .with_codec(EnvelopeCodec);
        for _ in 0..TOTAL / 128 {
            publisher
                .push_bulk(vec![Delivery::Job(job_id); 128])
                .await
                .unwrap();
        }
        let mut raw = transport.connection.clone();
        let scheduled: usize = redis::cmd("ZCARD")
            .arg(transport.config.scheduled_jobs_set())
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(scheduled, TOTAL);

        let processed = Arc::new(AtomicUsize::new(0));
        let stopping = CancellationToken::new();
        let handler = {
            let processed = Arc::clone(&processed);
            let stopping = stopping.clone();
            move |received| {
                let processed = Arc::clone(&processed);
                let stopping = stopping.clone();
                async move {
                    assert_eq!(received, job_id);
                    if processed.fetch_add(1, Ordering::SeqCst) + 1 == TOTAL {
                        stopping.cancel();
                    }
                    Ok(())
                }
            }
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            transport.run(stopping, MAX_CONCURRENCY, handler),
        )
        .await;
        let finished = processed.load(Ordering::SeqCst);
        remove_test_keys(&transport).await;
        outcome
            .expect("large backlog did not drain within its deadline")
            .expect("large backlog broke the Redis fetch script");
        assert_eq!(finished, TOTAL);
    }

    #[tokio::test]
    #[ignore = "requires TEST_REDIS_URL pointing to a disposable Redis"]
    async fn redis_orphan_recovery_is_bounded_resumable_and_preserves_live_workers() {
        let transport = test_transport().await;
        let config = &transport.config;
        let mut raw = transport.connection.clone();
        let first = WorkerContext::new("orphan-first");
        let second = WorkerContext::new("orphan-second");
        let live = WorkerContext::new("live-worker");
        let mut fixture = redis::pipe();
        fixture
            .cmd("ZADD")
            .arg(config.workers_set())
            .arg(10)
            .arg(first.name())
            .arg(20)
            .arg(second.name())
            .arg(1_000)
            .arg(live.name())
            .ignore()
            .cmd("SADD")
            .arg(config.inflight_set_for(&first))
            .arg(&["first-1", "first-2", "first-3", "first-4", "first-5"])
            .ignore()
            .cmd("SADD")
            .arg(config.inflight_set_for(&second))
            .arg(&["second-1", "second-2"])
            .ignore()
            .cmd("SADD")
            .arg(config.inflight_set_for(&live))
            .arg(&["live-1", "live-2", "live-3"])
            .ignore();
        for index in 0..10 {
            fixture
                .cmd("ZADD")
                .arg(config.workers_set())
                .arg(30 + index)
                .arg(format!("expired-empty-{index}"))
                .ignore();
        }
        fixture.query_async::<()>(&mut raw).await.unwrap();
        let script = redis::Script::new(include_str!(
            "../../vendor/apalis-redis/lua/reenqueue_orphaned.lua"
        ));
        let recover = |only: &str| {
            let mut invocation = script.prepare_invoke();
            invocation
                .key(config.workers_set())
                .key(config.active_jobs_list())
                .key(config.signal_list())
                .key(config.inflight_jobs_set())
                .arg(100)
                .arg(only)
                .arg(3);
            invocation
        };
        assert_eq!(
            recover(first.name())
                .invoke_async::<usize>(&mut raw)
                .await
                .unwrap(),
            3
        );
        let reserved: usize = redis::cmd("SCARD")
            .arg(config.inflight_set_for(&first))
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(reserved, 2);
        let registration: Option<f64> = redis::cmd("ZSCORE")
            .arg(config.workers_set())
            .arg(first.name())
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(registration, Some(10.0));
        let mut recovered = 3;
        for _ in 0..8 {
            let count = recover("").invoke_async::<usize>(&mut raw).await.unwrap();
            assert!(count <= 3);
            recovered += count;
        }
        assert_eq!(recovered, 7);
        assert_eq!(
            recover(live.name())
                .invoke_async::<usize>(&mut raw)
                .await
                .unwrap(),
            0
        );
        let remaining_workers: Vec<String> = redis::cmd("ZRANGE")
            .arg(config.workers_set())
            .arg(0)
            .arg(-1)
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(remaining_workers, [live.name()]);
        let live_jobs: usize = redis::cmd("SCARD")
            .arg(config.inflight_set_for(&live))
            .query_async(&mut raw)
            .await
            .unwrap();
        assert_eq!(live_jobs, 3);
        let mut recovered_jobs: Vec<String> = redis::cmd("LRANGE")
            .arg(config.active_jobs_list())
            .arg(0)
            .arg(-1)
            .query_async(&mut raw)
            .await
            .unwrap();
        recovered_jobs.sort_unstable();
        assert_eq!(
            recovered_jobs,
            [
                "first-1", "first-2", "first-3", "first-4", "first-5", "second-1", "second-2"
            ]
        );
        remove_test_keys(&transport).await;
    }
}
