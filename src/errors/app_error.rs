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

impl AppError {
    /// Stable machine-readable identifier. Never derive a public code from an
    /// upstream error message or SQL diagnostic.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::Validation(_) => "VALIDATION_FAILED",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Forbidden => "FORBIDDEN",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Conflict(_) => "CONFLICT",
            Self::TooManyRequests => "RATE_LIMITED",
            Self::Unavailable => "SERVICE_UNAVAILABLE",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::UnsupportedMediaType => "UNSUPPORTED_MEDIA_TYPE",
            Self::RequestTimeout => "REQUEST_TIMEOUT",
            Self::MethodNotAllowed => "METHOD_NOT_ALLOWED",
            Self::Database(_) | Self::Internal(_) => "INTERNAL_ERROR",
        }
    }

    pub const fn status(&self) -> StatusCode {
        match self {
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
        }
    }

    fn log_failure(&self) {
        // Never log arbitrary error Display/Debug output. SQL statements,
        // driver messages and anyhow chains may contain credentials or PII.
        match self {
            Self::Database(error) => tracing::error!(
                target: "rust_api_skeleton::errors",
                error_code = self.code(),
                error_class = "database",
                sqlstate = ?error.as_database_error().and_then(|cause| cause.code()),
                "HTTP request failed"
            ),
            Self::Internal(error) => {
                tracing::error!(
                    target: "rust_api_skeleton::errors",
                    error_code = self.code(),
                    error_class = "internal",
                    "HTTP request failed"
                );
                // Backtraces describe the call site, not the potentially
                // sensitive text in an anyhow error chain. They are only
                // emitted when the debug filter is enabled.
                tracing::debug!(
                    target: "rust_api_skeleton::errors",
                    error_code = self.code(),
                    backtrace = %error.backtrace(),
                    "Internal error backtrace"
                );
            }
            _ => {}
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        self.log_failure();
        let details = match &self {
            Self::Validation(errors) => Some(errors.clone()),
            _ => None,
        };
        let mut response =
            (status, response::error(self.code(), self.to_string(), details)).into_response();
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

    #[test]
    fn public_error_codes_and_http_statuses_are_stable() {
        let cases = [
            (AppError::BadRequest("bad request".into()), StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            (AppError::Validation(serde_json::json!({})), StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_FAILED"),
            (AppError::Unauthorized, StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            (AppError::Forbidden, StatusCode::FORBIDDEN, "FORBIDDEN"),
            (AppError::NotFound("missing".into()), StatusCode::NOT_FOUND, "NOT_FOUND"),
            (AppError::Conflict("already exists".into()), StatusCode::CONFLICT, "CONFLICT"),
            (AppError::TooManyRequests, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED"),
            (AppError::Unavailable, StatusCode::SERVICE_UNAVAILABLE, "SERVICE_UNAVAILABLE"),
            (AppError::PayloadTooLarge, StatusCode::PAYLOAD_TOO_LARGE, "PAYLOAD_TOO_LARGE"),
            (AppError::UnsupportedMediaType, StatusCode::UNSUPPORTED_MEDIA_TYPE, "UNSUPPORTED_MEDIA_TYPE"),
            (AppError::RequestTimeout, StatusCode::REQUEST_TIMEOUT, "REQUEST_TIMEOUT"),
            (AppError::MethodNotAllowed, StatusCode::METHOD_NOT_ALLOWED, "METHOD_NOT_ALLOWED"),
            (AppError::Database(sqlx::Error::RowNotFound), StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            (AppError::Internal(anyhow::anyhow!("hidden")), StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        ];
        for (error, expected_status, expected_code) in cases {
            assert_eq!(error.status(), expected_status, "{error:?}");
            assert_eq!(error.code(), expected_code, "{error:?}");
        }
    }

    #[tokio::test]
    async fn database_errors_never_expose_driver_details() {
        let response = AppError::Database(sqlx::Error::RowNotFound).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["code"], "INTERNAL_ERROR");
        assert_eq!(value["message"], "Database operation failed");
        assert!(value.get("errors").is_none());
    }

    #[tokio::test]
    async fn internal_error_never_exposes_cause_to_client() {
        let response =
            AppError::Internal(anyhow::anyhow!("database_password=sensitive")).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["message"], "Internal server error");
        assert_eq!(value["code"], "INTERNAL_ERROR");
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
        assert_eq!(value["code"], "VALIDATION_FAILED");
        assert!(value["errors"]["email"].is_array());
        assert!(value.get("data").is_none());
    }
}
