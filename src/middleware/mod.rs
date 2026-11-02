pub mod auth;
pub mod cors;
pub mod rate_limit;

use crate::{errors::AppError, state::AppState};
use axum::{
    extract::{Request, State},
    http::{HeaderValue, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub async fn observe_http(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    state.metrics.http_started();
    let response = next.run(request).await;
    state
        .metrics
        .http_finished(response.status(), started.elapsed());
    response
}

pub async fn request_deadline(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    match tokio::time::timeout(
        std::time::Duration::from_secs(state.settings.app.request_timeout_secs),
        next.run(request),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => AppError::RequestTimeout.into_response(),
    }
}

pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
