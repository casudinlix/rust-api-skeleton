//! Durable, at-least-once jobs shared by the API, worker and scheduler.
//!
//! A lease token fences queue state transitions. External effects such as SMTP
//! delivery still need idempotency at the destination to be exactly once.

mod backend;
mod backoff;
pub mod maintenance;
pub mod outbox;
mod queue;
pub mod redis;
pub mod relay;
pub mod send_welcome_email;
mod validation;
mod worker;

pub use backend::{JobQueue, queue_backend};
pub use queue::{DatabaseQueue, ReservedJob};
pub use worker::{run_worker, run_worker_with_probe};
