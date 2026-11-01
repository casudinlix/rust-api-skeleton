use std::{path::Path, time::Duration};

/// Development-only process supervision. Production should use an init system.
pub struct Options {
    pub scheduler: bool,
    pub shutdown_timeout: Duration,
}

#[cfg(unix)]
mod unix;

pub async fn run(root: &Path, options: Options) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        unix::run(root, options).await
    }
    #[cfg(not(unix))]
    {
        let _ = (root, options.scheduler, options.shutdown_timeout);
        anyhow::bail!(
            "artisan serve requires Unix process groups; use WSL or run API and worker separately"
        )
    }
}
