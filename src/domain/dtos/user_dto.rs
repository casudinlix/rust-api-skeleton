use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::{Validate, ValidationError};

use crate::domain::models::user::User;

use super::Normalize;

#[derive(Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct UpdateUserRequest {
    #[validate(length(min = 1, max = 100, message = "must contain 1 to 100 characters"))]
    #[validate(custom(function = "validate_name"))]
    pub name: String,
    #[validate(email(message = "must be a valid email address"))]
    #[validate(length(max = 254, message = "must contain at most 254 characters"))]
    pub email: String,
}

impl Normalize for UpdateUserRequest {
    fn normalize(&mut self) {
        self.name = self.name.trim().to_owned();
        self.email = self.email.trim().to_lowercase();
    }
}

#[derive(Clone, Serialize)]
pub struct UserResponse {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            user_id: user.user_id,
            name: user.name,
            email: user.email,
            role: user.role,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserListQuery {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize)]
pub struct UserListResponse {
    pub items: Vec<UserResponse>,
    pub next_cursor: Option<String>,
}

pub(super) fn validate_name(name: &str) -> Result<(), ValidationError> {
    if name.chars().any(char::is_control) {
        return Err(ValidationError::new("control_characters")
            .with_message("must not contain control characters".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dtos::normalize_and_validate;

    #[test]
    fn update_rejects_privileged_fields_and_control_characters() {
        let invalid = serde_json::json!({
            "name": "Ada",
            "email": "ada@example.com",
            "role": "admin",
        });
        assert!(serde_json::from_value::<UpdateUserRequest>(invalid).is_err());
        assert!(
            normalize_and_validate(UpdateUserRequest {
                name: "Ada\nLovelace".into(),
                email: "ada@example.com".into(),
            })
            .is_err()
        );
    }

    #[test]
    fn user_output_omits_password_material() {
        let now = Utc::now();
        let user = User {
            user_id: Uuid::now_v7(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
            role: "user".into(),
            created_at: now,
            updated_at: now,
        };
        let encoded = serde_json::to_value(UserResponse::from(user)).unwrap();
        assert!(encoded.get("user_id").is_some());
        assert!(encoded.get("id").is_none());
        assert!(encoded.get("password").is_none());
        assert!(encoded.get("password_hash").is_none());
        assert!(!encoded.to_string().contains("private-password-hash"));
    }
}
