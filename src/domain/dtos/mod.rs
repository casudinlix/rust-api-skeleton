pub mod auth_dto;
pub mod user_dto;

use serde_json::{Map, Value, json};
use validator::{Validate, ValidationErrors, ValidationErrorsKind};

use crate::errors::{AppError, AppResult};

pub use auth_dto::{LoginRequest, RegisterRequest, TokenResponse};
pub use user_dto::{UpdateUserRequest, UserListQuery, UserListResponse, UserResponse};

/// Normalize user-controlled text before applying validation rules.
/// Passwords must never be normalized, trimmed, or case-folded.
pub trait Normalize {
    fn normalize(&mut self);
}

/// Services validate as well as HTTP extractors so non-HTTP callers cannot bypass
/// the same normalization and validation rules.
pub fn normalize_and_validate<T: Normalize + Validate>(mut input: T) -> AppResult<T> {
    input.normalize();
    input
        .validate()
        .map_err(|errors| AppError::Validation(validation_error_details(&errors)))?;
    Ok(input)
}

/// Validator's normal serialization includes the rejected field value in
/// `params.value`. Build an allowlisted representation instead; passwords,
/// addresses, and other request values must not appear in error metadata.
pub fn validation_error_details(errors: &ValidationErrors) -> Value {
    let fields = errors
        .errors()
        .iter()
        .map(|(field, error)| {
            let details = match error {
                ValidationErrorsKind::Field(errors) => Value::Array(
                    errors
                        .iter()
                        .map(|error| match &error.message {
                            Some(message) => json!({
                                "code": error.code,
                                "message": message,
                            }),
                            None => json!({ "code": error.code }),
                        })
                        .collect(),
                ),
                ValidationErrorsKind::Struct(errors) => validation_error_details(errors),
                ValidationErrorsKind::List(entries) => Value::Object(
                    entries
                        .iter()
                        .map(|(index, errors)| {
                            (index.to_string(), validation_error_details(errors))
                        })
                        .collect(),
                ),
            };
            (field.to_string(), details)
        })
        .collect::<Map<String, Value>>();
    Value::Object(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_precedes_validation_without_changing_password() {
        let input = RegisterRequest {
            name: "  Ada Lovelace  ".into(),
            email: "  ADA@EXAMPLE.COM  ".into(),
            password: "  correct horse  ".into(),
        };
        let input = normalize_and_validate(input).expect("valid normalized input");
        assert_eq!(input.name, "Ada Lovelace");
        assert_eq!(input.email, "ada@example.com");
        assert_eq!(input.password, "  correct horse  ");
    }

    #[test]
    fn invalid_values_are_absent_from_validation_details() {
        let input = RegisterRequest {
            name: " ".into(),
            email: "private-invalid-address".into(),
            password: "secret".into(),
        };
        let Err(AppError::Validation(details)) = normalize_and_validate(input) else {
            panic!("invalid input must produce safe validation details");
        };
        let encoded = details.to_string();
        assert!(details.get("name").is_some());
        assert!(details.get("email").is_some());
        assert!(details.get("password").is_some());
        assert!(!encoded.contains("private-invalid-address"));
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("params"));
    }
}
