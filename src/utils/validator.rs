use axum::{
    Json,
    extract::{FromRequest, Request},
    http::StatusCode,
};
use serde::de::DeserializeOwned;
use validator::Validate;

use crate::{
    domain::dtos::{Normalize, normalize_and_validate},
    errors::AppError,
};

pub struct ValidatedJson<T>(pub T);

impl<S, T> FromRequest<S> for ValidatedJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Normalize + Validate + Send,
{
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(request, state)
            .await
            .map_err(|rejection| match rejection.status() {
                StatusCode::PAYLOAD_TOO_LARGE => AppError::PayloadTooLarge,
                StatusCode::UNSUPPORTED_MEDIA_TYPE => AppError::UnsupportedMediaType,
                _ => AppError::BadRequest("Invalid JSON body or unknown request fields".into()),
            })?;
        Ok(Self(normalize_and_validate(value)?))
    }
}
