use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use thiserror::Error;

use crate::{filesystem::StorageError, response};

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("Validation failed")]
    Validation(Value),
    #[error("Authentication required or token invalid")]
    Unauthorized,
    #[error("You do not have permission to perform this action")]
    Forbidden,
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("Too many requests; please try again later")]
    TooManyRequests,
    #[error("Service temporarily unavailable")]
    Unavailable,
    #[error("Request body is too large")]
    PayloadTooLarge,
    #[error("Content-Type must be application/json")]
    UnsupportedMediaType,
    #[error("Request timed out")]
    RequestTimeout,
    #[error("Method not allowed")]
    MethodNotAllowed,
    #[error("Database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("Internal server error")]
    Internal(#[from] anyhow::Error),
}

impl From<StorageError> for AppError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::InvalidPath => Self::BadRequest("Invalid storage path".into()),
            StorageError::TooLarge => Self::PayloadTooLarge,
            StorageError::NotFound => Self::NotFound("Stored file not found".into()),
            StorageError::Io => Self::Internal(anyhow::anyhow!("storage_io_failed")),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Database(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        match &self {
            Self::Database(error) => tracing::error!(
                error_class = "database",
                code = ?error.as_database_error().and_then(|e| e.code()),
                "database request failed"
            ),
            Self::Internal(_) => {
                tracing::error!(error_class = "internal", "application request failed")
            }
            _ => {}
        }
        let details = match &self {
            Self::Validation(errors) => Some(errors.clone()),
            _ => None,
        };
        let mut response = (status, response::error(self.to_string(), details)).into_response();
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                "Bearer".parse().expect("static header"),
            );
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, "60".parse().expect("static header"));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn internal_error_never_exposes_cause_to_client() {
        let response =
            AppError::Internal(anyhow::anyhow!("database_password=sensitive")).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["message"], "Internal server error");
        assert_eq!(value["success"], false);
        assert!(value.get("data").is_none());
        assert!(value.get("errors").is_none());
        assert!(!String::from_utf8_lossy(&bytes).contains("sensitive"));
    }

    #[tokio::test]
    async fn validation_error_uses_the_standard_error_envelope() {
        let response = AppError::Validation(serde_json::json!({
            "email": [{"code": "email", "message": "must be valid"}]
        }))
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["success"], false);
        assert_eq!(value["message"], "Validation failed");
        assert!(value["errors"]["email"].is_array());
        assert!(value.get("data").is_none());
    }
}
