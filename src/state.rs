use std::sync::Arc;

use crate::{
    config::Settings,
    database::DatabasePool,
    jobs::DatabaseQueue,
    middleware::rate_limit::AuthRateLimiter,
    repositories,
    services::{AuthService, UserService},
};

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub database: DatabasePool,
    pub auth_service: Arc<AuthService>,
    pub user_service: Arc<UserService>,
    pub queue: Arc<DatabaseQueue>,
    pub auth_rate_limiter: Arc<AuthRateLimiter>,
}

impl AppState {
    pub fn new(settings: Settings, database: DatabasePool) -> anyhow::Result<Self> {
        let users = repositories::user_repository(&database);
        let auth_service = Arc::new(AuthService::new(
            users.clone(),
            settings.jwt.clone(),
            settings.app.password_hash_concurrency,
        )?);
        let user_service = Arc::new(UserService::new(users));
        let queue = Arc::new(DatabaseQueue::new(database.clone(), settings.queue.clone()));
        let auth_rate_limiter =
            Arc::new(AuthRateLimiter::new(settings.app.auth_requests_per_minute));
        Ok(Self {
            settings: Arc::new(settings),
            database,
            auth_service,
            user_service,
            queue,
            auth_rate_limiter,
        })
    }
}
