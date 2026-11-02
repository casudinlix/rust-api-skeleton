use crate::{
    errors::{AppError, AppResult},
    response::{self, ApiJson},
    state::AppState,
};
use axum::extract::State;
use serde_json::{Value, json};

/// Process liveness deliberately does not depend on database availability.
pub async fn health() -> ApiJson<Value> {
    response::success("Service is alive", json!({"status": "ok"}))
}

pub async fn ready(State(state): State<AppState>) -> AppResult<ApiJson<Value>> {
    tokio::time::timeout(
        std::time::Duration::from_secs(state.settings.database.acquire_timeout_secs),
        state.database.ready(),
    )
    .await
    .map_err(|_| AppError::Unavailable)?
    .map_err(|_| AppError::Unavailable)?;
    Ok(response::success(
        "Service is ready",
        json!({"status": "ready", "database": "ok"}),
    ))
}
