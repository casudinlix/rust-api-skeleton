use clap::Parser;
use rust_api_skeleton::config::QueueDriver;

#[derive(Parser)]
#[command(about = "Run a durable database or Redis queue worker")]
struct Options {
    /// Override this worker's backend to drain an existing backlog.
    #[arg(long)]
    driver: Option<QueueDriver>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_api_skeleton::bootstrap::run_worker_with_driver(Options::parse().driver).await
}
