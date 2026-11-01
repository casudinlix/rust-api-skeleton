#[path = "../console/mod.rs"]
mod console;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    console::run().await
}
