pub mod cache;
pub mod database;
pub mod filesystem;
pub mod logging;
pub mod observability;
pub mod queue;
pub mod swagger;

pub use queue::{QueueConfig, QueueDriver, RedisQueueConfig};

use anyhow::{Context, ensure};
use cache::CacheConfig;
use database::DatabaseConfig;
use filesystem::FilesystemConfig;
use logging::LoggingConfig;
use observability::ObservabilityConfig;
use serde::Deserialize;
use swagger::SwaggerConfig;

#[derive(Clone)]
pub struct Settings {
    pub app: AppConfig,
    pub database: DatabaseConfig,
    pub cache: CacheConfig,
    pub filesystem: FilesystemConfig,
    pub logging: LoggingConfig,
    pub jwt: JwtConfig,
    pub cors: CorsConfig,
    pub queue: QueueConfig,
    pub scheduler: SchedulerConfig,
    pub mail: MailConfig,
    pub observability: ObservabilityConfig,
    pub swagger: SwaggerConfig,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub environment: String,
    pub log_format: String,
    pub request_timeout_secs: u64,
    pub shutdown_timeout_secs: u64,
    pub body_limit_bytes: usize,
    pub password_hash_concurrency: usize,
    pub auth_requests_per_minute: u32,
}

// Intentionally no Debug: secrets must not enter startup logs or error reports.
#[derive(Clone)]
pub struct JwtConfig {
    pub secret: String,
    pub issuer: String,
    pub audience: String,
    pub ttl_secs: u64,
}

#[derive(Clone, Debug)]
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    pub maintenance_cron: String,
    pub retention_days: u32,
}

#[derive(Clone)]
pub struct MailConfig {
    pub transport: String,
    pub from: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
}

#[derive(Deserialize)]
#[serde(default)]
struct RawSettings {
    app_env: String,
    app_host: String,
    app_port: u16,
    log_format: String,
    log_output: String,
    log_file_dir: String,
    log_file_prefix: String,
    log_rotation: String,
    log_max_files: usize,
    log_channel_capacity: usize,
    filesystem_root: String,
    filesystem_max_file_bytes: usize,
    request_timeout_secs: u64,
    shutdown_timeout_secs: u64,
    body_limit_bytes: usize,
    password_hash_concurrency: usize,
    auth_requests_per_minute: u32,
    observability_host: String,
    observability_api_port: u16,
    observability_worker_port: u16,
    observability_scheduler_port: u16,
    observability_relay_port: u16,
    swagger_enabled: bool,
    swagger_ui_path: String,
    swagger_openapi_path: String,
    swagger_asset_base_url: String,
    db_driver: String,
    database_url: String,
    db_max_connections: u32,
    db_min_connections: u32,
    db_acquire_timeout_secs: u64,
    db_idle_timeout_secs: u64,
    cache_driver: String,
    redis_url: String,
    cache_prefix: String,
    cache_default_ttl_secs: u64,
    cache_timeout_ms: u64,
    cache_max_value_bytes: usize,
    jwt_secret: String,
    jwt_issuer: String,
    jwt_audience: String,
    jwt_ttl_secs: u64,
    cors_allowed_origins: String,
    queue_poll_interval_ms: u64,
    queue_visibility_timeout_secs: u64,
    queue_job_timeout_secs: u64,
    queue_max_attempts: u32,
    queue_retry_base_secs: u64,
    queue_driver: String,
    queue_concurrency: usize,
    queue_redis_url: String,
    queue_redis_prefix: String,
    queue_redis_timeout_ms: u64,
    queue_relay_enabled: bool,
    scheduler_maintenance_cron: String,
    scheduler_retention_days: u32,
    mail_transport: String,
    mail_from: String,
    smtp_host: String,
    smtp_port: u16,
    smtp_username: String,
    smtp_password: String,
}

impl Default for RawSettings {
    fn default() -> Self {
        Self {
            app_env: "local".into(),
            app_host: "0.0.0.0".into(),
            app_port: 3000,
            log_format: "json".into(),
            log_output: "stdout".into(),
            log_file_dir: "storage/logs".into(),
            log_file_prefix: "app".into(),
            log_rotation: "daily".into(),
            log_max_files: 14,
            log_channel_capacity: 8_192,
            filesystem_root: "storage/app".into(),
            filesystem_max_file_bytes: 104_857_600,
            request_timeout_secs: 30,
            shutdown_timeout_secs: 45,
            body_limit_bytes: 65_536,
            password_hash_concurrency: 4,
            auth_requests_per_minute: 20,
            observability_host: "127.0.0.1".into(),
            observability_api_port: 9100,
            observability_worker_port: 9101,
            observability_scheduler_port: 9102,
            observability_relay_port: 9103,
            swagger_enabled: false,
            swagger_ui_path: "/docs".into(),
            swagger_openapi_path: "/api-docs/openapi.json".into(),
            swagger_asset_base_url: "https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.32.6".into(),
            db_driver: "postgres".into(),
            database_url: String::new(),
            db_max_connections: 10,
            db_min_connections: 1,
            db_acquire_timeout_secs: 5,
            db_idle_timeout_secs: 300,
            cache_driver: "none".into(),
            redis_url: String::new(),
            cache_prefix: "rust-api-skeleton:cache:".into(),
            cache_default_ttl_secs: 300,
            cache_timeout_ms: 500,
            cache_max_value_bytes: 1_048_576,
            jwt_secret: String::new(),
            jwt_issuer: "rust-api-skeleton".into(),
            jwt_audience: "rust-api-clients".into(),
            jwt_ttl_secs: 900,
            cors_allowed_origins: "http://localhost:5173".into(),
            queue_poll_interval_ms: 1000,
            queue_visibility_timeout_secs: 90,
            queue_job_timeout_secs: 30,
            queue_max_attempts: 5,
            queue_retry_base_secs: 5,
            queue_driver: "database".into(),
            queue_concurrency: 1,
            queue_redis_url: String::new(),
            queue_redis_prefix: "rust-api-skeleton:queue:v1".into(),
            queue_redis_timeout_ms: 2_000,
            queue_relay_enabled: true,
            scheduler_maintenance_cron: "0 */5 * * * *".into(),
            scheduler_retention_days: 7,
            mail_transport: "log".into(),
            mail_from: "API <noreply@example.com>".into(),
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_username: String::new(),
            smtp_password: String::new(),
        }
    }
}

impl Settings {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_with_queue_driver(None)
    }

    /// Process-local override; never mutates global environment after Tokio starts.
    pub fn load_with_queue_driver(driver: Option<QueueDriver>) -> anyhow::Result<Self> {
        match dotenvy::dotenv() {
            Ok(_) => {}
            Err(dotenvy::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => anyhow::bail!("could not load .env; check its syntax and permissions"),
        }
        let mut builder =
            config::Config::builder().add_source(config::Environment::default().try_parsing(true));
        if let Some(driver) = driver {
            builder = builder
                .set_override("queue_driver", driver.to_string())
                .map_err(|_| anyhow::anyhow!("could not override queue driver"))?;
        }
        let source = builder
            .build()
            .map_err(|_| anyhow::anyhow!("could not read environment configuration"))?;
        Self::from_config(source)
    }

    /// Supports deterministic configuration in tests without mutating process env.
    pub fn from_config(source: config::Config) -> anyhow::Result<Self> {
        // Config deserialization errors can contain raw source values, so do not
        // attach the underlying error (it may contain a secret).
        let raw: RawSettings = source
            .try_deserialize()
            .map_err(|_| anyhow::anyhow!("invalid setting type; check .env.example"))?;
        let settings = Self {
            app: AppConfig {
                host: raw.app_host,
                port: raw.app_port,
                environment: raw.app_env,
                log_format: raw.log_format,
                request_timeout_secs: raw.request_timeout_secs,
                shutdown_timeout_secs: raw.shutdown_timeout_secs,
                body_limit_bytes: raw.body_limit_bytes,
                password_hash_concurrency: raw.password_hash_concurrency,
                auth_requests_per_minute: raw.auth_requests_per_minute,
            },
            observability: ObservabilityConfig {
                host: raw
                    .observability_host
                    .parse()
                    .context("invalid OBSERVABILITY_HOST")?,
                api_port: raw.observability_api_port,
                worker_port: raw.observability_worker_port,
                scheduler_port: raw.observability_scheduler_port,
                relay_port: raw.observability_relay_port,
            },
            swagger: SwaggerConfig {
                enabled: raw.swagger_enabled,
                ui_path: raw.swagger_ui_path,
                openapi_path: raw.swagger_openapi_path,
                asset_base_url: raw.swagger_asset_base_url,
            },
            filesystem: FilesystemConfig {
                root: raw.filesystem_root.into(),
                max_file_bytes: raw.filesystem_max_file_bytes,
            },
            logging: LoggingConfig {
                output: raw.log_output.parse().context("invalid LOG_OUTPUT")?,
                directory: raw.log_file_dir.into(),
                file_prefix: raw.log_file_prefix,
                rotation: raw.log_rotation.parse().context("invalid LOG_ROTATION")?,
                max_files: raw.log_max_files,
                channel_capacity: raw.log_channel_capacity,
            },
            database: DatabaseConfig {
                driver: raw.db_driver.parse().context("invalid DB_DRIVER")?,
                url: raw.database_url,
                max_connections: raw.db_max_connections,
                min_connections: raw.db_min_connections,
                acquire_timeout_secs: raw.db_acquire_timeout_secs,
                idle_timeout_secs: raw.db_idle_timeout_secs,
            },
            jwt: JwtConfig {
                secret: raw.jwt_secret,
                issuer: raw.jwt_issuer,
                audience: raw.jwt_audience,
                ttl_secs: raw.jwt_ttl_secs,
            },
            cache: CacheConfig {
                driver: raw.cache_driver.parse().context("invalid CACHE_DRIVER")?,
                redis_url: raw.redis_url,
                prefix: raw.cache_prefix,
                default_ttl_secs: raw.cache_default_ttl_secs,
                timeout_ms: raw.cache_timeout_ms,
                max_value_bytes: raw.cache_max_value_bytes,
            },
            cors: CorsConfig {
                allowed_origins: raw
                    .cors_allowed_origins
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            },
            queue: QueueConfig {
                driver: raw.queue_driver.parse().context("invalid QUEUE_DRIVER")?,
                poll_interval_ms: raw.queue_poll_interval_ms,
                visibility_timeout_secs: raw.queue_visibility_timeout_secs,
                job_timeout_secs: raw.queue_job_timeout_secs,
                max_attempts: raw.queue_max_attempts,
                retry_base_secs: raw.queue_retry_base_secs,
                concurrency: raw.queue_concurrency,
                redis: RedisQueueConfig {
                    url: raw.queue_redis_url,
                    prefix: raw.queue_redis_prefix,
                    timeout_ms: raw.queue_redis_timeout_ms,
                },
                relay_enabled: raw.queue_relay_enabled,
            },
            scheduler: SchedulerConfig {
                maintenance_cron: raw.scheduler_maintenance_cron,
                retention_days: raw.scheduler_retention_days,
            },
            mail: MailConfig {
                transport: raw.mail_transport,
                from: raw.mail_from,
                smtp_host: raw.smtp_host,
                smtp_port: raw.smtp_port,
                smtp_username: raw.smtp_username,
                smtp_password: raw.smtp_password,
            },
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.database.validate()?;
        self.cache.validate()?;
        self.filesystem.validate()?;
        self.logging.validate()?;
        self.observability.validate()?;
        self.swagger.validate()?;
        ensure!(
            [
                self.observability.api_port,
                self.observability.worker_port,
                self.observability.scheduler_port,
                self.observability.relay_port,
            ]
            .iter()
            .all(|port| *port != self.app.port),
            "observability ports must not reuse APP_PORT"
        );
        self.queue.validate()?;
        ensure!(
            self.app.host.parse::<std::net::IpAddr>().is_ok(),
            "APP_HOST must be an IP address"
        );
        ensure!(self.app.port > 0, "APP_PORT must be nonzero");
        ensure!(
            ["local", "test", "staging", "production"].contains(&self.app.environment.as_str()),
            "APP_ENV must be local, test, staging or production"
        );
        ensure!(
            ["json", "pretty"].contains(&self.app.log_format.as_str()),
            "LOG_FORMAT must be json or pretty"
        );
        ensure!(
            (1..=300).contains(&self.app.request_timeout_secs),
            "REQUEST_TIMEOUT_SECS must be 1..300"
        );
        ensure!(
            (1..=600).contains(&self.app.shutdown_timeout_secs),
            "SHUTDOWN_TIMEOUT_SECS must be 1..600"
        );
        ensure!(
            (1024..=10_485_760).contains(&self.app.body_limit_bytes),
            "BODY_LIMIT_BYTES must be 1024..10485760"
        );
        ensure!(
            (1..=64).contains(&self.app.password_hash_concurrency),
            "PASSWORD_HASH_CONCURRENCY must be 1..64"
        );
        ensure!(
            (1..=10_000).contains(&self.app.auth_requests_per_minute),
            "AUTH_REQUESTS_PER_MINUTE must be 1..10000"
        );
        let secret = self.jwt.secret.trim();
        ensure!(
            secret.len() >= 32
                && secret.len() <= 4096
                && !secret.to_ascii_lowercase().contains("replace")
                && !secret.to_ascii_lowercase().contains("change-me"),
            "JWT_SECRET must be a generated secret of at least 32 bytes; use openssl rand -hex 32"
        );
        ensure!(
            !self.jwt.issuer.trim().is_empty() && !self.jwt.audience.trim().is_empty(),
            "JWT_ISSUER and JWT_AUDIENCE must be nonempty"
        );
        ensure!(
            (60..=86_400).contains(&self.jwt.ttl_secs),
            "JWT_TTL_SECS must be 60..86400"
        );
        for origin in &self.cors.allowed_origins {
            let parsed = url::Url::parse(origin)
                .map_err(|_| anyhow::anyhow!("CORS_ALLOWED_ORIGINS contains an invalid origin"))?;
            ensure!(
                ["http", "https"].contains(&parsed.scheme())
                    && parsed.host_str().is_some()
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && parsed.query().is_none()
                    && parsed.fragment().is_none()
                    && parsed.origin().ascii_serialization() == *origin,
                "CORS_ALLOWED_ORIGINS must be comma-separated HTTP(S) origins without paths, credentials or wildcards"
            );
        }
        ensure!(
            self.app.shutdown_timeout_secs > self.queue.job_timeout_secs
                && self.app.shutdown_timeout_secs > self.app.request_timeout_secs,
            "SHUTDOWN_TIMEOUT_SECS must exceed request and job timeouts"
        );
        if self.queue.driver == QueueDriver::Redis {
            // Final transport ACK, Apalis close, then two bounded cleanup calls.
            // The process deadline still bounds slow SQL and failed connections.
            let cleanup_budget = (self.queue.redis.timeout_ms * 4).div_ceil(1_000);
            ensure!(
                self.app.shutdown_timeout_secs > self.queue.job_timeout_secs + cleanup_budget,
                "SHUTDOWN_TIMEOUT_SECS must exceed job timeout plus four Redis operation timeouts"
            );
        }
        ensure!(
            (1..=3650).contains(&self.scheduler.retention_days),
            "SCHEDULER_RETENTION_DAYS must be 1..3650"
        );
        tokio_cron_scheduler::Job::new_async(self.scheduler.maintenance_cron.as_str(), |_, _| {
            Box::pin(async {})
        })
        .map_err(|_| {
            anyhow::anyhow!(
                "SCHEDULER_MAINTENANCE_CRON must be a valid cron expression including seconds"
            )
        })?;
        ensure!(
            ["log", "smtp"].contains(&self.mail.transport.as_str()),
            "MAIL_TRANSPORT must be log or smtp"
        );
        self.mail
            .from
            .parse::<lettre::message::Mailbox>()
            .map_err(|_| anyhow::anyhow!("MAIL_FROM must be a valid mailbox"))?;
        if self.mail.transport == "smtp" {
            ensure!(
                !self.mail.smtp_host.trim().is_empty() && self.mail.smtp_port > 0,
                "SMTP_HOST and SMTP_PORT are required for SMTP"
            );
            ensure!(
                self.mail.smtp_username.is_empty() == self.mail.smtp_password.is_empty(),
                "SMTP_USERNAME and SMTP_PASSWORD must both be set or both be empty"
            );
        }
        if self.app.environment == "production" {
            ensure!(
                self.mail.transport == "smtp",
                "production requires MAIL_TRANSPORT=smtp; log transport does not deliver email"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        let (driver, url) = if cfg!(feature = "postgres") {
            ("postgres", "postgres://api:password@localhost/app")
        } else {
            ("mysql", "mysql://api:password@localhost/app")
        };
        let source = config::Config::builder()
            .set_override("db_driver", driver)
            .unwrap()
            .set_override("database_url", url)
            .unwrap()
            .set_override(
                "jwt_secret",
                "test-only-secret-with-more-than-thirty-two-bytes",
            )
            .unwrap()
            .build()
            .unwrap();
        Settings::from_config(source).unwrap()
    }

    #[test]
    fn filesystem_and_logging_configuration_are_validated() {
        let mut s = settings();
        s.filesystem.root = "../outside".into();
        assert!(s.validate().is_err());

        s.filesystem.root = "storage/app".into();
        s.logging.file_prefix = "bad prefix".into();
        assert!(s.validate().is_err());

        s.logging.file_prefix = "app".into();
        s.logging.max_files = 14;
        s.validate().unwrap();
    }

    #[test]
    fn swagger_is_disabled_by_default_and_paths_are_validated() {
        let mut s = settings();
        assert!(!s.swagger.enabled);
        s.swagger.ui_path = "../ignored-while-disabled".into();
        s.swagger.asset_base_url.clear();
        s.validate().unwrap();

        s.swagger.enabled = true;
        s.swagger.ui_path = "/api/v1/docs".into();
        assert!(s.validate().is_err());
        s.swagger.ui_path = "/docs".into();
        s.swagger.openapi_path = "/api-docs/openapi.json".into();
        s.swagger.asset_base_url = "https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.32.6".into();
        s.validate().unwrap();
    }

    #[test]
    fn observability_ports_must_be_nonzero_and_distinct() {
        let mut s = settings();
        s.observability.worker_port = s.observability.api_port;
        assert!(s.validate().is_err());
        s.observability.worker_port = 9101;
        s.observability.relay_port = 0;
        assert!(s.validate().is_err());
        s.observability.relay_port = 9103;
        s.observability.api_port = s.app.port;
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_placeholder_secret() {
        let mut s = settings();
        s.jwt.secret = "replace-with-output-of-openssl-rand-hex-32".into();
        assert!(s.validate().unwrap_err().to_string().contains("JWT_SECRET"));
    }

    #[test]
    fn lease_must_outlive_job_deadline() {
        let mut s = settings();
        s.queue.visibility_timeout_secs = s.queue.job_timeout_secs;
        assert!(s.validate().is_err());
    }

    #[test]
    fn redis_shutdown_reserves_time_for_transport_ack_and_cleanup() {
        let mut s = settings();
        s.queue.driver = QueueDriver::Redis;
        s.queue.redis.url = "redis://127.0.0.1:6379/0".into();
        s.queue.redis.timeout_ms = 2_001;
        s.app.shutdown_timeout_secs = s.queue.job_timeout_secs + 8;
        assert!(s.validate().is_err());
        s.app.shutdown_timeout_secs = s.queue.job_timeout_secs + 10;
        s.validate().unwrap();
    }

    #[test]
    fn cors_rejects_wildcards_and_paths() {
        for origin in [
            "*",
            "https://example.com/path",
            "https://user:pass@example.com",
            "https://example.com/",
        ] {
            let mut s = settings();
            s.cors.allowed_origins = vec![origin.into()];
            assert!(s.validate().is_err(), "accepted {origin}");
        }
    }

    #[test]
    fn production_requires_delivery_transport() {
        let mut s = settings();
        s.app.environment = "production".into();
        assert!(
            s.validate()
                .unwrap_err()
                .to_string()
                .contains("MAIL_TRANSPORT")
        );
    }

    #[test]
    fn invalid_numeric_config_does_not_echo_source_values() {
        let source = config::Config::builder()
            .set_override("jwt_ttl_secs", "secret-value-must-not-leak")
            .unwrap()
            .build()
            .unwrap();
        let error = Settings::from_config(source).err().unwrap().to_string();
        assert!(!error.contains("secret-value"));
    }

    #[test]
    fn rejects_invalid_cache_configuration_without_echoing_source_values() {
        let driver = if cfg!(feature = "postgres") {
            "postgres"
        } else {
            "mysql"
        };
        let database_url = format!("{driver}://test:test@localhost/test");
        for (name, value) in [
            ("cache_driver", "unsupported-secret-value"),
            ("cache_prefix", "invalid prefix secret-value"),
            ("cache_default_ttl_secs", "0"),
            ("cache_timeout_ms", "0"),
            ("cache_max_value_bytes", "0"),
        ] {
            let source = config::Config::builder()
                .set_override("db_driver", driver)
                .unwrap()
                .set_override("database_url", database_url.clone())
                .unwrap()
                .set_override("jwt_secret", "test-only-key-longer-than-thirty-two-bytes")
                .unwrap()
                .set_override(name, value)
                .unwrap()
                .build()
                .unwrap();
            let error = Settings::from_config(source)
                .err()
                .unwrap_or_else(|| panic!("accepted invalid {name}"));
            assert!(!format!("{error:#}").contains("secret-value"));
        }
    }

    #[test]
    fn redis_cache_requires_an_explicit_url() {
        let driver = if cfg!(feature = "postgres") {
            "postgres"
        } else {
            "mysql"
        };
        let source = config::Config::builder()
            .set_override("db_driver", driver)
            .unwrap()
            .set_override(
                "database_url",
                format!("{driver}://test:test@localhost/test"),
            )
            .unwrap()
            .set_override("jwt_secret", "test-only-key-longer-than-thirty-two-bytes")
            .unwrap()
            .set_override("cache_driver", "redis")
            .unwrap()
            .build()
            .unwrap();
        assert!(Settings::from_config(source).is_err());
    }
}
