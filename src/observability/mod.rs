use std::{
    fmt::Write as _,
    future::Future,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::{
    config::observability::ObservabilityConfig,
    database::{DatabasePool, DatabasePoolStats},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessKind {
    Api,
    Worker,
    Scheduler,
    Relay,
}

impl ProcessKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Worker => "worker",
            Self::Scheduler => "scheduler",
            Self::Relay => "relay",
        }
    }

    const fn port(self, config: &ObservabilityConfig) -> u16 {
        match self {
            Self::Api => config.api_port,
            Self::Worker => config.worker_port,
            Self::Scheduler => config.scheduler_port,
            Self::Relay => config.relay_port,
        }
    }
}

#[derive(Clone, Default)]
pub struct ProcessProbe {
    metrics: Metrics,
    ready: Arc<AtomicBool>,
}

impl ProcessProbe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics.clone()
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Release);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}

#[derive(Clone, Default)]
pub struct Metrics {
    inner: Arc<MetricsInner>,
}

#[derive(Default)]
struct MetricsInner {
    http_requests_total: AtomicU64,
    http_in_flight: AtomicU64,
    http_duration_micros_total: AtomicU64,
    http_duration_count: AtomicU64,
    http_2xx_total: AtomicU64,
    http_3xx_total: AtomicU64,
    http_4xx_total: AtomicU64,
    http_5xx_total: AtomicU64,
    http_other_total: AtomicU64,
    jobs_started_total: AtomicU64,
    jobs_in_flight: AtomicU64,
    jobs_completed_total: AtomicU64,
    jobs_failed_total: AtomicU64,
    jobs_lease_lost_total: AtomicU64,
    jobs_transition_error_total: AtomicU64,
    jobs_duration_micros_total: AtomicU64,
    jobs_duration_count: AtomicU64,
    queue_claim_errors_total: AtomicU64,
    redis_reconnects_total: AtomicU64,
    relay_published_total: AtomicU64,
    relay_publish_failed_total: AtomicU64,
    scheduler_enqueued_total: AtomicU64,
    scheduler_enqueue_failed_total: AtomicU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobOutcome {
    Completed,
    HandlerFailed,
    LeaseLost,
    TransitionError,
}

impl Metrics {
    pub fn http_started(&self) {
        self.inner.http_in_flight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn http_finished(&self, status: StatusCode, elapsed: Duration) {
        self.inner.http_requests_total.fetch_add(1, Ordering::Relaxed);
        self.inner
            .http_in_flight
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        self.inner
            .http_duration_micros_total
            .fetch_add(duration_micros(elapsed), Ordering::Relaxed);
        self.inner
            .http_duration_count
            .fetch_add(1, Ordering::Relaxed);
        let counter = match status.as_u16() / 100 {
            2 => &self.inner.http_2xx_total,
            3 => &self.inner.http_3xx_total,
            4 => &self.inner.http_4xx_total,
            5 => &self.inner.http_5xx_total,
            _ => &self.inner.http_other_total,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn job_started(&self) {
        self.inner.jobs_started_total.fetch_add(1, Ordering::Relaxed);
        self.inner.jobs_in_flight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn job_finished(&self, outcome: JobOutcome, elapsed: Duration) {
        self.inner
            .jobs_in_flight
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        self.inner
            .jobs_duration_micros_total
            .fetch_add(duration_micros(elapsed), Ordering::Relaxed);
        self.inner
            .jobs_duration_count
            .fetch_add(1, Ordering::Relaxed);
        match outcome {
            JobOutcome::Completed => {
                self.inner.jobs_completed_total.fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::HandlerFailed => {
                self.inner.jobs_failed_total.fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::LeaseLost => {
                self.inner.jobs_lease_lost_total.fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::TransitionError => {
                self.inner
                    .jobs_transition_error_total
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn queue_claim_failed(&self) {
        self.inner
            .queue_claim_errors_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn redis_reconnect(&self) {
        self.inner.redis_reconnects_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn relay_published(&self) {
        self.inner.relay_published_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn relay_publish_failed(&self) {
        self.inner
            .relay_publish_failed_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn scheduler_enqueued(&self) {
        self.inner.scheduler_enqueued_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn scheduler_enqueue_failed(&self) {
        self.inner
            .scheduler_enqueue_failed_total
            .fetch_add(1, Ordering::Relaxed);
    }

    fn render(&self, process: ProcessKind, pool: DatabasePoolStats, ready: bool) -> String {
        let label = process.as_str();
        let mut output = String::with_capacity(4096);

        metric_help_type(
            &mut output,
            "rust_api_process_ready",
            "Whether the process dependency readiness probe is currently healthy",
            "gauge",
        );
        metric(
            &mut output,
            "rust_api_process_ready",
            label,
            if ready { 1 } else { 0 },
        );

        metric_help_type(
            &mut output,
            "rust_api_database_pool_connections",
            "SQLx pool connections by state",
            "gauge",
        );
        metric_state(
            &mut output,
            "rust_api_database_pool_connections",
            label,
            "open",
            u64::from(pool.open),
        );
        metric_state(
            &mut output,
            "rust_api_database_pool_connections",
            label,
            "idle",
            u64::from(pool.idle),
        );
        metric_state(
            &mut output,
            "rust_api_database_pool_connections",
            label,
            "active",
            u64::from(pool.active),
        );

        metric_help_type(
            &mut output,
            "rust_api_http_requests_total",
            "HTTP responses completed by this process",
            "counter",
        );
        metric(
            &mut output,
            "rust_api_http_requests_total",
            label,
            self.inner.http_requests_total.load(Ordering::Relaxed),
        );
        metric_help_type(
            &mut output,
            "rust_api_http_requests_in_flight",
            "HTTP requests currently executing",
            "gauge",
        );
        metric(
            &mut output,
            "rust_api_http_requests_in_flight",
            label,
            self.inner.http_in_flight.load(Ordering::Relaxed),
        );
        metric_help_type(
            &mut output,
            "rust_api_http_responses_total",
            "HTTP responses by bounded status class",
            "counter",
        );
        for (class, value) in [
            ("2xx", self.inner.http_2xx_total.load(Ordering::Relaxed)),
            ("3xx", self.inner.http_3xx_total.load(Ordering::Relaxed)),
            ("4xx", self.inner.http_4xx_total.load(Ordering::Relaxed)),
            ("5xx", self.inner.http_5xx_total.load(Ordering::Relaxed)),
            ("other", self.inner.http_other_total.load(Ordering::Relaxed)),
        ] {
            metric_state(&mut output, "rust_api_http_responses_total", label, class, value);
        }
        summary(
            &mut output,
            "rust_api_http_request_duration_seconds",
            "HTTP request duration",
            label,
            self.inner.http_duration_micros_total.load(Ordering::Relaxed),
            self.inner.http_duration_count.load(Ordering::Relaxed),
        );

        for (name, help, kind, value) in [
            (
                "rust_api_worker_jobs_started_total",
                "Jobs accepted for execution",
                "counter",
                self.inner.jobs_started_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_worker_jobs_in_flight",
                "Jobs currently executing",
                "gauge",
                self.inner.jobs_in_flight.load(Ordering::Relaxed),
            ),
            (
                "rust_api_worker_jobs_completed_total",
                "Jobs whose completion transition committed",
                "counter",
                self.inner.jobs_completed_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_worker_job_failures_total",
                "Job handler failures before queue retry or terminal handling",
                "counter",
                self.inner.jobs_failed_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_worker_lease_lost_total",
                "Stale job completions rejected by queue fencing",
                "counter",
                self.inner.jobs_lease_lost_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_worker_transition_errors_total",
                "Job outcome transitions that could not be durably recorded",
                "counter",
                self.inner.jobs_transition_error_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_queue_claim_errors_total",
                "Queue acquisition errors",
                "counter",
                self.inner.queue_claim_errors_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_redis_reconnects_total",
                "Redis queue reconnect attempts after transport failures",
                "counter",
                self.inner.redis_reconnects_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_relay_published_total",
                "Outbox deliveries published to Redis",
                "counter",
                self.inner.relay_published_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_relay_publish_failed_total",
                "Outbox Redis publish failures",
                "counter",
                self.inner.relay_publish_failed_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_scheduler_enqueued_total",
                "Scheduled maintenance jobs enqueued",
                "counter",
                self.inner.scheduler_enqueued_total.load(Ordering::Relaxed),
            ),
            (
                "rust_api_scheduler_enqueue_failed_total",
                "Scheduled maintenance enqueue failures",
                "counter",
                self.inner.scheduler_enqueue_failed_total.load(Ordering::Relaxed),
            ),
        ] {
            metric_help_type(&mut output, name, help, kind);
            metric(&mut output, name, label, value);
        }
        summary(
            &mut output,
            "rust_api_worker_job_duration_seconds",
            "Worker job execution and durable outcome duration",
            label,
            self.inner.jobs_duration_micros_total.load(Ordering::Relaxed),
            self.inner.jobs_duration_count.load(Ordering::Relaxed),
        );
        output
    }
}

fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn metric_help_type(output: &mut String, name: &str, help: &str, kind: &str) {
    writeln!(output, "# HELP {name} {help}").expect("write to String");
    writeln!(output, "# TYPE {name} {kind}").expect("write to String");
}

fn metric(output: &mut String, name: &str, process: &str, value: u64) {
    writeln!(output, "{name}{{process=\"{process}\"}} {value}").expect("write to String");
}

fn metric_state(output: &mut String, name: &str, process: &str, state: &str, value: u64) {
    writeln!(
        output,
        "{name}{{process=\"{process}\",state=\"{state}\"}} {value}"
    )
    .expect("write to String");
}

fn summary(
    output: &mut String,
    name: &str,
    help: &str,
    process: &str,
    micros: u64,
    count: u64,
) {
    metric_help_type(output, name, help, "summary");
    writeln!(
        output,
        "{name}_sum{{process=\"{process}\"}} {:.6}",
        micros as f64 / 1_000_000.0
    )
    .expect("write to String");
    writeln!(output, "{name}_count{{process=\"{process}\"}} {count}")
        .expect("write to String");
}

#[derive(Clone)]
struct ServerState {
    process: ProcessKind,
    probe: ProcessProbe,
    database: DatabasePool,
    readiness_timeout: Duration,
}

#[derive(Serialize)]
struct HealthBody {
    status: &'static str,
    process: &'static str,
}

#[derive(Serialize)]
struct ReadyBody {
    status: &'static str,
    process: &'static str,
    database: &'static str,
    backend: &'static str,
}

async fn health(State(state): State<ServerState>) -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        process: state.process.as_str(),
    })
}

async fn ready(State(state): State<ServerState>) -> (StatusCode, Json<ReadyBody>) {
    let database_ok = matches!(
        tokio::time::timeout(state.readiness_timeout, state.database.ready()).await,
        Ok(Ok(()))
    );
    let backend_ok = state.probe.is_ready();
    let ready = database_ok && backend_ok;
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(ReadyBody {
            status: if ready { "ready" } else { "not_ready" },
            process: state.process.as_str(),
            database: if database_ok { "ok" } else { "unavailable" },
            backend: if backend_ok { "ok" } else { "unavailable" },
        }),
    )
}

async fn metrics(State(state): State<ServerState>) -> Response {
    let body = state
        .probe
        .metrics
        .render(state.process, state.database.stats(), state.probe.is_ready());
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        body,
    )
        .into_response()
}

async fn serve(
    config: ObservabilityConfig,
    process: ProcessKind,
    probe: ProcessProbe,
    database: DatabasePool,
    cancellation: CancellationToken,
    readiness_timeout: Duration,
) -> anyhow::Result<()> {
    let address = SocketAddr::new(config.host, process.port(&config));
    let state = ServerState {
        process,
        probe,
        database,
        readiness_timeout,
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/metrics", get(metrics))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("could not bind {} observability listener", process.as_str()))?;
    tracing::info!(%address, process = process.as_str(), "observability listener started");
    axum::serve(listener, app)
        .with_graceful_shutdown(cancellation.cancelled_owned())
        .await
        .context("observability server stopped unexpectedly")
}

pub async fn supervise<F>(
    service: F,
    config: ObservabilityConfig,
    process: ProcessKind,
    probe: ProcessProbe,
    database: DatabasePool,
    cancellation: CancellationToken,
    readiness_timeout: Duration,
) -> anyhow::Result<()>
where
    F: Future<Output = anyhow::Result<()>>,
{
    let server = serve(
        config,
        process,
        probe.clone(),
        database,
        cancellation.clone(),
        readiness_timeout,
    );
    tokio::pin!(service, server);
    tokio::select! {
        service_result = &mut service => {
            probe.set_ready(false);
            cancellation.cancel();
            let server_result = server.await;
            server_result?;
            service_result
        }
        server_result = &mut server => {
            probe.set_ready(false);
            cancellation.cancel();
            let service_result = service.await;
            server_result?;
            service_result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_probe_is_not_ready_until_the_service_marks_it_ready() {
        let probe = ProcessProbe::new();
        assert!(!probe.is_ready());
        probe.set_ready(true);
        assert!(probe.is_ready());
        probe.set_ready(false);
        assert!(!probe.is_ready());
    }

    #[test]
    fn metrics_use_bounded_labels_and_prometheus_text_shape() {
        let metrics = Metrics::default();
        metrics.http_started();
        metrics.http_finished(StatusCode::CREATED, Duration::from_millis(12));
        metrics.job_started();
        metrics.job_finished(JobOutcome::Completed, Duration::from_millis(8));
        let output = metrics.render(
            ProcessKind::Api,
            DatabasePoolStats {
                open: 3,
                idle: 2,
                active: 1,
            },
            true,
        );
        assert!(output.contains("rust_api_http_requests_total{process=\"api\"} 1"));
        assert!(output.contains(
            "rust_api_http_responses_total{process=\"api\",state=\"2xx\"} 1"
        ));
        assert!(output.contains("rust_api_database_pool_connections{process=\"api\",state=\"active\"} 1"));
        assert!(output.contains("rust_api_worker_jobs_completed_total{process=\"api\"} 1"));
        assert!(!output.contains("path="));
        assert!(!output.contains("user_id"));
    }

    #[test]
    fn process_ports_are_stable_and_distinct() {
        let config = ObservabilityConfig {
            host: "127.0.0.1".parse().unwrap(),
            api_port: 9100,
            worker_port: 9101,
            scheduler_port: 9102,
            relay_port: 9103,
        };
        config.validate().unwrap();
        assert_eq!(ProcessKind::Api.port(&config), 9100);
        assert_eq!(ProcessKind::Worker.port(&config), 9101);
        assert_eq!(ProcessKind::Scheduler.port(&config), 9102);
        assert_eq!(ProcessKind::Relay.port(&config), 9103);
    }
}
