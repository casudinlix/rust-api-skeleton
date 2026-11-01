//! Durable, at-least-once jobs shared by the API, worker and scheduler.
//!
//! A lease token fences queue state transitions. External effects such as SMTP
//! delivery still need idempotency at the destination to be exactly once.

mod backoff;
pub mod maintenance;
mod queue;
pub mod send_welcome_email;
mod worker;

pub use queue::{DatabaseQueue, ReservedJob};
pub use worker::run_worker;
