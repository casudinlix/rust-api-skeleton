use crate::{
    domain::dtos::{LoginRequest, RegisterRequest, TokenResponse},
    errors::AppResult,
    response::{self, ApiJson, ApiStatusJson},
    state::AppState,
    utils::validator::ValidatedJson,
};
use axum::extract::State;

pub async fn register(
    State(state): State<AppState>,
    ValidatedJson(input): ValidatedJson<RegisterRequest>,
) -> AppResult<ApiStatusJson<TokenResponse>> {
    let token = state.auth_service.register(input).await?;
    Ok(response::created("Registration successful", token))
}

pub async fn login(
    State(state): State<AppState>,
    ValidatedJson(input): ValidatedJson<LoginRequest>,
) -> AppResult<ApiJson<TokenResponse>> {
    let token = state.auth_service.login(input).await?;
    Ok(response::success("Login successful", token))
}
