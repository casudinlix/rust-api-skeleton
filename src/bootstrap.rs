use anyhow::Context;
use std::{
    future::{Future, IntoFuture},
    net::SocketAddr,
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    config::{QueueDriver, Settings},
    database::DatabasePool,
    jobs, routes, schedulers,
    state::AppState,
    utils::shutdown::shutdown_signal,
};

fn init_tracing(settings: &Settings) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info"));
    // Third-party persistence diagnostics can include raw serialized jobs or
    // server text. Our queue adapter emits safe categories and application IDs.
    let safe_targets = tracing_subscriber::filter::filter_fn(|metadata| {
        !metadata.target().starts_with("apalis_core")
            && !metadata.target().starts_with("apalis_redis")
    });
    if settings.app.log_format == "json" {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_filter(safe_targets),
            )
            .try_init()
            .map_err(|_| anyhow::anyhow!("could not initialize JSON tracing"))?;
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_filter(safe_targets))
            .try_init()
            .map_err(|_| anyhow::anyhow!("could not initialize tracing"))?;
    }
    Ok(())
}

async fn initialize() -> anyhow::Result<(Settings, DatabasePool)> {
    initialize_with_queue_driver(None).await
}

async fn initialize_with_queue_driver(
    driver: Option<QueueDriver>,
) -> anyhow::Result<(Settings, DatabasePool)> {
    let settings = Settings::load_with_queue_driver(driver)?;
    init_tracing(&settings)?;
    let database = DatabasePool::connect(&settings.database).await?;
    Ok((settings, database))
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
    let (settings, database) = initialize().await?;
    let grace = settings.app.shutdown_timeout_secs;
    let address = SocketAddr::new(settings.app.host.parse()?, settings.app.port);
    let cancellation = CancellationToken::new();
    let shutdown = cancellation.clone();
    let task = async {
        let state = AppState::new(settings, database.clone())?;
        let app = routes::api::router(state)?;
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .context("could not bind API listener")?;
        tracing::info!(%address, "API listening");
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .into_future()
        .await
        .context("HTTP server stopped unexpectedly")
    };
    run_service(task, &database, cancellation, grace).await
}

pub async fn run_worker() -> anyhow::Result<()> {
    run_worker_with_driver(None).await
}

pub async fn run_worker_with_driver(driver: Option<QueueDriver>) -> anyhow::Result<()> {
    let (settings, database) = initialize_with_queue_driver(driver).await?;
    let cancellation = CancellationToken::new();
    let task = jobs::run_worker(
        database.clone(),
        settings.queue,
        settings.mail,
        cancellation.clone(),
    );
    run_service(
        task,
        &database,
        cancellation,
        settings.app.shutdown_timeout_secs,
    )
    .await
}

pub async fn run_relay() -> anyhow::Result<()> {
    let (settings, database) = initialize_with_queue_driver(Some(QueueDriver::Redis)).await?;
    let cancellation = CancellationToken::new();
    let task = jobs::relay::run_relay(database.clone(), settings.queue, cancellation.clone());
    run_service(
        task,
        &database,
        cancellation,
        settings.app.shutdown_timeout_secs,
    )
    .await
}

pub async fn run_scheduler() -> anyhow::Result<()> {
    let (settings, database) = initialize().await?;
    let cancellation = CancellationToken::new();
    let queue = jobs::queue_backend(&database, &settings.queue);
    let task = schedulers::run_scheduler(queue, settings.scheduler, cancellation.clone());
    run_service(
        task,
        &database,
        cancellation,
        settings.app.shutdown_timeout_secs,
    )
    .await
}

pub async fn run_migrations() -> anyhow::Result<()> {
    let (settings, database) = initialize().await?;
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
