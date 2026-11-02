use crate::{
    domain::dtos::{
        RegisterRequest, UpdateUserRequest, UserListQuery, UserResponse,
    },
    errors::{AppError, AppResult},
    middleware::auth::{AdminUser, AuthUser},
    state::AppState,
    utils::{response::{ApiCursorResponse, ApiResponse}, validator::ValidatedJson},
};
use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::StatusCode,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub async fn me(AuthUser(user): AuthUser) -> Json<ApiResponse<UserResponse>> {
    Json(ApiResponse::success("Profile retrieved", user.into()))
}

pub async fn update_me(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ValidatedJson(input): ValidatedJson<UpdateUserRequest>,
) -> AppResult<Json<ApiResponse<UserResponse>>> {
    Ok(Json(ApiResponse::success(
        "Profile updated",
        state.user_service.update(user.user_id, input).await?,
    )))
}

pub async fn create(
    State(state): State<AppState>,
    _admin: AdminUser,
    ValidatedJson(input): ValidatedJson<RegisterRequest>,
) -> AppResult<(StatusCode, Json<ApiResponse<UserResponse>>)> {
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::success(
            "User created",
            state.auth_service.create_user(input).await?,
        )),
    ))
}

pub async fn list(
    State(state): State<AppState>,
    _admin: AdminUser,
    query: Result<Query<UserListQuery>, QueryRejection>,
) -> AppResult<Json<ApiCursorResponse<UserResponse>>> {
    let Query(query) =
        query.map_err(|_| AppError::BadRequest("Invalid pagination query".into()))?;
    let page = state.user_service.list(query).await?;
    Ok(Json(ApiCursorResponse::success(
        "Users retrieved",
        page.items,
        page.next_cursor,
    )))
}

pub async fn show(
    State(state): State<AppState>,
    _admin: AdminUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> AppResult<Json<ApiResponse<UserResponse>>> {
    let user_id = parse_user_id(path)?;
    Ok(Json(ApiResponse::success(
        "User retrieved",
        state.user_service.get(user_id).await?,
    )))
}

pub async fn update(
    State(state): State<AppState>,
    _admin: AdminUser,
    path: Result<Path<Uuid>, PathRejection>,
    ValidatedJson(input): ValidatedJson<UpdateUserRequest>,
) -> AppResult<Json<ApiResponse<UserResponse>>> {
    let user_id = parse_user_id(path)?;
    Ok(Json(ApiResponse::success(
        "User updated",
        state.user_service.update(user_id, input).await?,
    )))
}

pub async fn delete(
    State(state): State<AppState>,
    admin: AdminUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let user_id = parse_user_id(path)?;
    if user_id == admin.0.user_id {
        return Err(AppError::Conflict(
            "An administrator cannot delete their own account".into(),
        ));
    }
    state.user_service.delete(user_id).await?;
    Ok(Json(ApiResponse::success(
        "User deleted",
        json!({"deleted": true}),
    )))
}

fn parse_user_id(path: Result<Path<Uuid>, PathRejection>) -> AppResult<Uuid> {
    path.map(|Path(user_id)| user_id)
        .map_err(|_| AppError::BadRequest("User ID must be a valid UUID".into()))
}
