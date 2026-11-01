use crate::{
    domain::dtos::{LoginRequest, RegisterRequest, TokenResponse},
    errors::AppResult,
    state::AppState,
    utils::{response::ApiResponse, validator::ValidatedJson},
};
use axum::{Json, extract::State, http::StatusCode};

pub async fn register(
    State(state): State<AppState>,
    ValidatedJson(input): ValidatedJson<RegisterRequest>,
) -> AppResult<(StatusCode, Json<ApiResponse<TokenResponse>>)> {
    let token = state.auth_service.register(input).await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::success("Registration successful", token)),
    ))
}

pub async fn login(
    State(state): State<AppState>,
    ValidatedJson(input): ValidatedJson<LoginRequest>,
) -> AppResult<Json<ApiResponse<TokenResponse>>> {
    let token = state.auth_service.login(input).await?;
    Ok(Json(ApiResponse::success("Login successful", token)))
}
