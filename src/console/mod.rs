mod doctor;
mod generate;
mod hooks;
mod install;
mod packages;
mod project;
mod serve;
mod storage;
mod templates;

use std::{path::PathBuf, time::Duration};

use clap::{Args, Parser, Subcommand, builder::BoolishValueParser};

#[derive(Parser)]
#[command(
    name = "artisan",
    version,
    about = "Laravel-style development commands for the Rust API skeleton"
)]
struct Cli {
    /// Project directory; defaults to the nearest parent containing Cargo.toml and src/lib.rs.
    #[arg(long, global = true, value_name = "PATH")]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize .env and JWT key, then fetch/build locked dependencies. Does not migrate.
    Install,
    /// Run runtime preflight checks for toolchain, configuration, dependencies and writable paths.
    Doctor,
    /// Configure this repository's versioned .githooks directory.
    #[command(name = "hooks:install")]
    HooksInstall {
        /// Replace an existing local core.hooksPath setting.
        #[arg(long, env = "ARTISAN_FORCE", value_parser = BoolishValueParser::new())]
        force: bool,
    },
    /// Build once and supervise API + worker on Unix. Ctrl+C stops all processes.
    Serve {
        /// Also start the task scheduler.
        #[arg(long, env = "ARTISAN_SCHEDULER", value_parser = BoolishValueParser::new())]
        scheduler: bool,
        /// Graceful shutdown deadline before forced cleanup; keep above application timeout.
        #[arg(long, env = "ARTISAN_SHUTDOWN_TIMEOUT", default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
        shutdown_timeout: u64,
    },
    /// Create public/storage -> configured FILESYSTEM_ROOT/public.
    #[command(name = "storage:link")]
    StorageLink {
        /// Override the link target; defaults to FILESYSTEM_ROOT/public.
        #[arg(long, value_name = "PATH")]
        target: Option<PathBuf>,
        /// Override the public link path; defaults to public/storage.
        #[arg(long, value_name = "PATH")]
        link: Option<PathBuf>,
        /// Replace an existing symlink that points elsewhere; never overwrites files/directories.
        #[arg(long, env = "ARTISAN_FORCE", value_parser = BoolishValueParser::new())]
        force: bool,
    },
    /// Generate a missing/placeholder JWT key. Explicit --force rotates a configured key.
    #[command(name = "key:generate")]
    Key {
        /// Rotate the key and invalidate tokens signed with the previous key.
        #[arg(long, env = "ARTISAN_FORCE", value_parser = BoolishValueParser::new())]
        force: bool,
    },
    /// Generate a JSON handler and register its module; wire routes explicitly.
    #[command(name = "make:controller")]
    Controller(Name),
    /// Generate a UUIDv7 model; NAME is an explicit singular entity (e.g. Person, Status).
    #[command(name = "make:model")]
    Model(Name),
    /// Generate a validated request DTO with normalization.
    #[command(name = "make:request", visible_alias = "make:dto")]
    Request(Name),
    /// Generate a service boundary.
    #[command(name = "make:service")]
    Service(Name),
    /// Generate a repository and both SQL adapters; NAME uses a singular entity.
    #[command(name = "make:repository")]
    Repository(Name),
    /// Generate a job payload and handler; wire dispatch explicitly.
    #[command(name = "make:job")]
    Job(Name),
    /// Generate a middleware scaffold; implement its policy before attaching it.
    #[command(name = "make:middleware")]
    Middleware(Name),
    /// Generate paired SQL stubs; document table/PK mapping and indexes before implementing both.
    #[command(name = "make:migration")]
    Migration(Name),
    /// Add a crates.io package with Cargo (e.g. serde@1).
    Require {
        #[command(flatten)]
        package: Package,
        /// Comma-separated dependency features.
        #[arg(long, env = "ARTISAN_FEATURES", hide_env_values = true)]
        features: Option<String>,
        /// Add to dev-dependencies.
        #[arg(long, env = "ARTISAN_DEV", value_parser = BoolishValueParser::new())]
        dev: bool,
    },
    /// Remove a dependency with Cargo.
    Remove {
        #[command(flatten)]
        package: Package,
        /// Remove from dev-dependencies.
        #[arg(long, env = "ARTISAN_DEV", value_parser = BoolishValueParser::new())]
        dev: bool,
    },
    /// Update Cargo.lock, optionally selecting one package.
    Update {
        #[arg(
            env = "ARTISAN_PACKAGE",
            hide_env_values = true,
            value_name = "PACKAGE"
        )]
        package: Option<String>,
    },
}

#[derive(Args)]
struct Name {
    #[arg(env = "ARTISAN_NAME", hide_env_values = true, value_name = "NAME")]
    name: String,
}

#[derive(Args)]
struct Package {
    #[arg(
        env = "ARTISAN_PACKAGE",
        hide_env_values = true,
        value_name = "PACKAGE"
    )]
    package: String,
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = project::discover(cli.root.as_deref())?;
    match cli.command {
        Command::Install => {
            {
                let _guard = project::lock(&root)?;
                report_environment(install::initialize(&root, false, false)?);
            }
            packages::fetch_and_build(&root)?;
            println!(
                "Project initialized. Configure .env, start your database, then run make migrate and make serve. Run make hooks-install once to enable the pre-commit gate."
            );
        }
        Command::Doctor => {
            let _guard = project::lock(&root)?;
            doctor::run(&root).await?;
        }
        Command::HooksInstall { force } => {
            hooks::install(&root, force)?;
            println!("Git hooks enabled from .githooks.");
        }
        Command::Serve {
            scheduler,
            shutdown_timeout,
        } => {
            serve::run(
                &root,
                serve::Options {
                    scheduler,
                    shutdown_timeout: Duration::from_secs(shutdown_timeout),
                },
            )
            .await?;
        }
        Command::StorageLink {
            target,
            link,
            force,
        } => {
            let _guard = project::lock(&root)?;
            storage::link(&root, target.as_deref(), link.as_deref(), force)?;
            println!("Public storage link is ready.");
        }
        Command::Key { force } => {
            let _guard = project::lock(&root)?;
            report_environment(install::initialize(&root, true, force)?);
        }
        Command::Require {
            package,
            features,
            dev,
        } => {
            let _guard = project::lock(&root)?;
            packages::require(&root, &package.package, features.as_deref(), dev)?;
        }
        Command::Remove { package, dev } => {
            let _guard = project::lock(&root)?;
            packages::remove(&root, &package.package, dev)?;
        }
        Command::Update { package } => {
            let _guard = project::lock(&root)?;
            packages::update(&root, package.as_deref())?;
        }
        command => {
            let (kind, name) = match command {
                Command::Controller(name) => (generate::Kind::Controller, name.name),
                Command::Model(name) => (generate::Kind::Model, name.name),
                Command::Request(name) => (generate::Kind::Request, name.name),
                Command::Service(name) => (generate::Kind::Service, name.name),
                Command::Repository(name) => (generate::Kind::Repository, name.name),
                Command::Job(name) => (generate::Kind::Job, name.name),
                Command::Middleware(name) => (generate::Kind::Middleware, name.name),
                Command::Migration(name) => (generate::Kind::Migration, name.name),
                _ => unreachable!("non-generator command was already handled"),
            };
            let _guard = project::lock(&root)?;
            for relative in generate::run(&root, kind, &name)? {
                println!("Created {}", relative.display());
            }
            if kind == generate::Kind::Migration {
                println!("Complete SQL in BOTH database files before running make migrate.");
            } else {
                println!(
                    "Rust modules registered. Implement the scaffold and wire routes, dependencies, or job dispatch explicitly."
                );
            }
        }
    }
    Ok(())
}

fn report_environment(status: install::EnvironmentStatus) {
    println!(
        "{}",
        match status {
            install::EnvironmentStatus::Created =>
                "Created .env with private permissions; no secrets are printed.",
            install::EnvironmentStatus::KeyGenerated =>
                "Generated a new JWT key in .env. Restart running processes to use it.",
            install::EnvironmentStatus::Preserved =>
                "Preserved existing .env settings and configured JWT key.",
        }
    );
}
