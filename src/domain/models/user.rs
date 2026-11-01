use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Minimal read projection for delivery handlers, without authentication data.
#[derive(Clone)]
pub struct UserContact {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
}

/// Public read projection. Credential material belongs only to `UserCredentials`.
#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Login-only projection, deliberately not serializable.
#[derive(Clone)]
pub struct UserCredentials {
    pub user: User,
    pub password_hash: String,
}

impl fmt::Debug for UserCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserCredentials")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct NewUser {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
    pub password_hash: String,
}

impl fmt::Debug for NewUser {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewUser")
            .field("user_id", &self.user_id)
            .field("name", &self.name)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct UserUpdate {
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserCursor {
    pub created_at: DateTime<Utc>,
    pub user_id: Uuid,
}

#[derive(Debug)]
pub struct UserPage {
    pub users: Vec<User>,
    pub next_cursor: Option<UserCursor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_projection_has_named_identifier_and_credentials_are_redacted() {
        let user = User {
            user_id: Uuid::now_v7(),
            name: "Example".to_owned(),
            email: "example@example.com".to_owned(),
            role: "user".to_owned(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let serialized = serde_json::to_value(&user).unwrap();
        assert!(serialized.get("user_id").is_some());
        assert!(serialized.get("id").is_none());
        assert!(serialized.get("password_hash").is_none());
        let debug = format!("{user:?}");
        assert!(!debug.contains("password_hash"));
        assert!(!debug.contains("sensitive-password-hash"));

        let credentials = UserCredentials {
            user: user.clone(),
            password_hash: "sensitive-password-hash".to_owned(),
        };
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("password_hash"));
        assert!(!debug.contains("sensitive-password-hash"));

        let new_user = NewUser {
            user_id: user.user_id,
            name: user.name,
            email: user.email,
            password_hash: credentials.password_hash,
        };
        let debug = format!("{new_user:?}");
        assert!(!debug.contains("password_hash"));
        assert!(!debug.contains("sensitive-password-hash"));
    }
}
