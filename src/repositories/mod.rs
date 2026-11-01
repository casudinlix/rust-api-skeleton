use std::sync::Arc;

use crate::{config::QueueDriver, database::DatabasePool};

#[cfg(feature = "mysql")]
pub mod mysql;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod user_repository;

pub use user_repository::UserRepository;

pub fn user_repository(pool: &DatabasePool) -> Arc<dyn UserRepository> {
    user_repository_with_queue(pool, QueueDriver::Database)
}

pub fn user_repository_with_queue(
    pool: &DatabasePool,
    driver: QueueDriver,
) -> Arc<dyn UserRepository> {
    match pool {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => Arc::new(
            postgres::user_pg_repo::PostgresUserRepository::with_queue_driver(pool.clone(), driver),
        ),
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => Arc::new(
            mysql::user_mysql_repo::MysqlUserRepository::with_queue_driver(pool.clone(), driver),
        ),
    }
}
