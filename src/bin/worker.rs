#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_api_skeleton::bootstrap::run_worker().await
}
