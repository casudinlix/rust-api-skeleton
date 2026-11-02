//! API-only application services shared by the HTTP, queue and scheduler binaries.

#[cfg(not(any(feature = "postgres", feature = "mysql")))]
compile_error!("enable at least one database feature: postgres or mysql");

pub mod bootstrap;
pub mod cache;
pub mod config;
pub mod database;
pub mod domain;
pub mod errors;
pub mod filesystem;
pub mod handlers;
pub mod jobs;
pub mod logging;
pub mod middleware;
pub mod observability;
pub mod repositories;
pub mod routes;
pub mod schedulers;
pub mod services;
pub mod state;
pub mod utils;

pub use utils::response;
