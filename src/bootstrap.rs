use crate::{
    config::{QueueDriver, Settings},
    database::DatabasePool,
    jobs, logging,
    observability::{self, ProcessKind, ProcessProbe},
    routes, schedulers,
    state::AppState,
    utils::shutdown::shutdown_signal,
};
use anyhow::Context;
use std::{
    future::{Future, IntoFuture},
    net::SocketAddr,
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

async fn initialize(
    process: &'static str,
) -> anyhow::Result<(Settings, DatabasePool, logging::LogGuard)> {
    initialize_with_queue_driver(process, None).await
}

async fn initialize_with_queue_driver(
    process: &'static str,
    driver: Option<QueueDriver>,
) -> anyhow::Result<(Settings, DatabasePool, logging::LogGuard)> {
    let settings = Settings::load_with_queue_driver(driver)?;
    let log_guard = logging::init(&settings, process)?;
    let database = DatabasePool::connect(&settings.database).await?;
    Ok((settings, database, log_guard))
}

struct DrainOutcome {
    result: anyhow::Result<()>,
    deadline: Instant,
}

/// Owns and drops the service future before the caller starts closing its pool.
/// The same deadline is carried forward; pool closure gets only remaining time.
async fn drain_on_signal<F, S>(
    task: F,
    signal: S,
    cancellation: CancellationToken,
    grace: Duration,
) -> DrainOutcome
where
    F: Future<Output = anyhow::Result<()>>,
    S: Future<Output = ()>,
{
    tokio::pin!(task);
    tokio::select! {
        result = &mut task => DrainOutcome { result, deadline: Instant::now() + grace },
        () = signal => {
            let deadline = Instant::now() + grace;
            cancellation.cancel();
            let result = tokio::time::timeout_at(deadline, &mut task).await
                .context("graceful service drain deadline exceeded")
                .and_then(|result| result);
            DrainOutcome { result, deadline }
        }
    }
}

async fn finish_shutdown<C>(outcome: DrainOutcome, close: C) -> anyhow::Result<()>
where
    C: Future<Output = ()>,
{
    let close_result = tokio::time::timeout_at(outcome.deadline, close)
        .await
        .context("database pool did not close within shutdown deadline");
    if close_result.is_err() {
        tracing::error!(
            error_class = "pool_close_timeout",
            "database close exceeded shutdown deadline"
        );
    }
    outcome.result.and(close_result)
}

async fn run_service<F>(
    task: F,
    database: &DatabasePool,
    cancellation: CancellationToken,
    grace_secs: u64,
) -> anyhow::Result<()>
where
    F: Future<Output = anyhow::Result<()>>,
{
    let outcome = drain_on_signal(
        task,
        shutdown_signal(),
        cancellation,
        Duration::from_secs(grace_secs),
    )
    .await;
    finish_shutdown(outcome, database.close()).await
}

pub async fn run_api() -> anyhow::Result<()> {
    let (settings, database, _log_guard) = initialize("api").await?;
    let grace = settings.app.shutdown_timeout_secs;
    let readiness_timeout = Duration::from_secs(settings.database.acquire_timeout_secs);
    let observability_config = settings.observability.clone();
    let address = SocketAddr::new(settings.app.host.parse()?, settings.app.port);
    let cancellation = CancellationToken::new();
    let shutdown = cancellation.clone();
    let probe = ProcessProbe::new();
    let metrics = probe.metrics();
    let service_probe = probe.clone();
    let task = async {
        let state = AppState::new_with_metrics(settings, database.clone(), metrics)?;
        let app = routes::api::router(state)?;
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .context("could not bind API listener")?;
        service_probe.set_ready(true);
        tracing::info!(%address, "API listening");
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .into_future()
        .await
        .context("HTTP server stopped unexpectedly");
        service_probe.set_ready(false);
        result
    };
    let task = observability::supervise(
        task,
        observability_config,
        ProcessKind::Api,
        probe,
        database.clone(),
        cancellation.clone(),
        readiness_timeout,
    );
    run_service(task, &database, cancellation, grace).await
}

pub async fn run_worker() -> anyhow::Result<()> {
    run_worker_with_driver(None).await
}

pub async fn run_worker_with_driver(driver: Option<QueueDriver>) -> anyhow::Result<()> {
    let (settings, database, _log_guard) = initialize_with_queue_driver("worker", driver).await?;
    let cancellation = CancellationToken::new();
    let probe = ProcessProbe::new();
    let readiness_timeout = Duration::from_secs(settings.database.acquire_timeout_secs);
    let observability_config = settings.observability.clone();
    let grace = settings.app.shutdown_timeout_secs;
    let task = jobs::run_worker_with_probe(
        database.clone(),
        settings.queue,
        settings.mail,
        cancellation.clone(),
        probe.clone(),
    );
    let task = observability::supervise(
        task,
        observability_config,
        ProcessKind::Worker,
        probe,
        database.clone(),
        cancellation.clone(),
        readiness_timeout,
    );
    run_service(task, &database, cancellation, grace).await
}

pub async fn run_relay() -> anyhow::Result<()> {
    let (settings, database, _log_guard) =
        initialize_with_queue_driver("relay", Some(QueueDriver::Redis)).await?;
    let cancellation = CancellationToken::new();
    let probe = ProcessProbe::new();
    let readiness_timeout = Duration::from_secs(settings.database.acquire_timeout_secs);
    let observability_config = settings.observability.clone();
    let grace = settings.app.shutdown_timeout_secs;
    let task = jobs::relay::run_relay_with_probe(
        database.clone(),
        settings.queue,
        cancellation.clone(),
        probe.clone(),
    );
    let task = observability::supervise(
        task,
        observability_config,
        ProcessKind::Relay,
        probe,
        database.clone(),
        cancellation.clone(),
        readiness_timeout,
    );
    run_service(task, &database, cancellation, grace).await
}

pub async fn run_scheduler() -> anyhow::Result<()> {
    let (settings, database, _log_guard) = initialize("scheduler").await?;
    let cancellation = CancellationToken::new();
    let probe = ProcessProbe::new();
    let readiness_timeout = Duration::from_secs(settings.database.acquire_timeout_secs);
    let observability_config = settings.observability.clone();
    let grace = settings.app.shutdown_timeout_secs;
    let queue = jobs::queue_backend(&database, &settings.queue);
    let task = schedulers::run_scheduler_with_probe(
        queue,
        settings.scheduler,
        cancellation.clone(),
        probe.clone(),
    );
    let task = observability::supervise(
        task,
        observability_config,
        ProcessKind::Scheduler,
        probe,
        database.clone(),
        cancellation.clone(),
        readiness_timeout,
    );
    run_service(task, &database, cancellation, grace).await
}

pub async fn run_migrations() -> anyhow::Result<()> {
    let (settings, database, _log_guard) = initialize("migrate").await?;
    tracing::info!(driver = %settings.database.driver, "applying embedded migrations");
    run_service(
        database.migrate(),
        &database,
        CancellationToken::new(),
        settings.app.shutdown_timeout_secs,
    )
    .await?;
    tracing::info!("migrations complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::pending,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    #[tokio::test]
    async fn pool_close_cannot_extend_an_expired_shutdown_deadline() {
        let outcome = DrainOutcome {
            result: Ok(()),
            deadline: Instant::now(),
        };
        let result =
            tokio::time::timeout(Duration::from_secs(1), finish_shutdown(outcome, pending()))
                .await
                .expect("unbounded pool close escaped the shutdown deadline");
        assert!(result.unwrap_err().to_string().contains("database pool"));
    }

    #[tokio::test]
    async fn signal_allows_cooperative_work_to_drain_before_close() {
        let cancellation = CancellationToken::new();
        let stopping = cancellation.clone();
        let drained = Arc::new(AtomicBool::new(false));
        let marker = drained.clone();
        let task = async move {
            stopping.cancelled().await;
            marker.store(true, Ordering::SeqCst);
            Ok(())
        };
        let outcome = drain_on_signal(task, async {}, cancellation, Duration::from_secs(1)).await;
        finish_shutdown(outcome, async {}).await.unwrap();
        assert!(drained.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn stalled_service_cannot_outlive_zero_grace() {
        let outcome = drain_on_signal(
            pending(),
            async {},
            CancellationToken::new(),
            Duration::ZERO,
        )
        .await;
        assert!(
            outcome
                .result
                .unwrap_err()
                .to_string()
                .contains("drain deadline")
        );
    }
}
