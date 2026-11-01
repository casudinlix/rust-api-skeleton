use std::sync::Arc;

use crate::{
    cache::Cache,
    config::Settings,
    database::DatabasePool,
    jobs::{JobQueue, queue_backend},
    middleware::rate_limit::AuthRateLimiter,
    repositories,
    services::{AuthService, UserService},
};

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub database: DatabasePool,
    pub cache: Arc<Cache>,
    pub auth_service: Arc<AuthService>,
    pub user_service: Arc<UserService>,
    pub queue: Arc<dyn JobQueue>,
    pub auth_rate_limiter: Arc<AuthRateLimiter>,
}

impl AppState {
    pub fn new(settings: Settings, database: DatabasePool) -> anyhow::Result<Self> {
        settings.queue.validate()?;
        let cache = Arc::new(Cache::from_config(&settings.cache)?);
        let users = repositories::user_repository_with_queue(&database, settings.queue.driver);
        let auth_service = Arc::new(AuthService::new(
            users.clone(),
            settings.jwt.clone(),
            settings.app.password_hash_concurrency,
        )?);
        let user_service = Arc::new(UserService::new(users));
        let queue = queue_backend(&database, &settings.queue);
        let auth_rate_limiter =
            Arc::new(AuthRateLimiter::new(settings.app.auth_requests_per_minute));
        Ok(Self {
            settings: Arc::new(settings),
            database,
            cache,
            auth_service,
            user_service,
            queue,
            auth_rate_limiter,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::cache::CacheDriver;

    #[tokio::test]
    async fn invalid_cache_configuration_cannot_be_silently_ignored_by_state() {
        let (driver, url) = if cfg!(feature = "postgres") {
            ("postgres", "postgres://test:test@127.0.0.1:9/test")
        } else {
            ("mysql", "mysql://test:test@127.0.0.1:9/test")
        };
        let mut settings = Settings::from_config(
            config::Config::builder()
                .set_override("db_driver", driver)
                .unwrap()
                .set_override("database_url", url)
                .unwrap()
                .set_override("jwt_secret", "test-only-key-longer-than-thirty-two-bytes")
                .unwrap()
                .build()
                .unwrap(),
        )
        .unwrap();
        settings.cache.driver = CacheDriver::Redis;
        settings.cache.redis_url.clear();
        #[cfg(feature = "postgres")]
        let database = DatabasePool::Postgres(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy(url)
                .unwrap(),
        );
        #[cfg(all(feature = "mysql", not(feature = "postgres")))]
        let database = DatabasePool::Mysql(
            sqlx::mysql::MySqlPoolOptions::new()
                .connect_lazy(url)
                .unwrap(),
        );
        assert!(AppState::new(settings, database).is_err());
    }
}
