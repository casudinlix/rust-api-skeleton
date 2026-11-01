use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use super::{Normalize, UserResponse};

/// Only client-editable fields are accepted. The database assigns role `user`.
#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    #[validate(length(min = 1, max = 100, message = "must contain 1 to 100 characters"))]
    #[validate(custom(function = "super::user_dto::validate_name"))]
    pub name: String,
    #[validate(email(message = "must be a valid email address"))]
    #[validate(length(max = 254, message = "must contain at most 254 characters"))]
    pub email: String,
    #[validate(custom(function = "validate_registration_password"))]
    pub password: String,
}

impl Normalize for RegisterRequest {
    fn normalize(&mut self) {
        self.name = self.name.trim().to_owned();
        self.email = self.email.trim().to_lowercase();
    }
}

#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    #[validate(email(message = "must be a valid email address"))]
    #[validate(length(max = 254, message = "must contain at most 254 characters"))]
    pub email: String,
    #[validate(custom(function = "validate_login_password"))]
    pub password: String,
}

impl Normalize for LoginRequest {
    fn normalize(&mut self) {
        self.email = self.email.trim().to_lowercase();
    }
}

// Intentionally no Debug or Deserialize: bearer tokens should not be logged and
// this type is only used for output.
#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub user: UserResponse,
}

fn validate_registration_password(password: &str) -> Result<(), ValidationError> {
    if !(12..=128).contains(&password.len()) {
        return Err(ValidationError::new("password_length")
            .with_message("must contain 12 to 128 bytes".into()));
    }
    Ok(())
}

fn validate_login_password(password: &str) -> Result<(), ValidationError> {
    if password.is_empty() || password.len() > 128 {
        return Err(ValidationError::new("password_length")
            .with_message("must contain 1 to 128 bytes".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dtos::normalize_and_validate;

    #[test]
    fn role_and_unknown_fields_are_rejected() {
        for field in ["role", "password_hash", "unexpected"] {
            let mut register = serde_json::json!({
                "name": "Ada",
                "email": "ada@example.com",
                "password": "correct horse battery",
            });
            register[field] = serde_json::json!("admin");
            assert!(serde_json::from_value::<RegisterRequest>(register).is_err());

            let mut login = serde_json::json!({
                "email": "ada@example.com",
                "password": "correct horse battery",
            });
            login[field] = serde_json::json!("admin");
            assert!(serde_json::from_value::<LoginRequest>(login).is_err());
        }
    }

    #[test]
    fn registration_password_bounds_count_bytes() {
        let make = |password: String| RegisterRequest {
            name: "Ada".into(),
            email: "ada@example.com".into(),
            password,
        };
        assert!(normalize_and_validate(make("a".repeat(11))).is_err());
        assert!(normalize_and_validate(make("a".repeat(12))).is_ok());
        assert!(normalize_and_validate(make("a".repeat(128))).is_ok());
        assert!(normalize_and_validate(make("a".repeat(129))).is_err());
        assert!(normalize_and_validate(make("é".repeat(65))).is_err());
    }
}
