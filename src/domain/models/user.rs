use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct User {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl fmt::Debug for User {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("User")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("email", &self.email)
            .field("role", &self.role)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct NewUser {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub password_hash: String,
}

impl fmt::Debug for NewUser {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewUser")
            .field("id", &self.id)
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
    pub id: Uuid,
}

#[derive(Debug)]
pub struct UserPage {
    pub users: Vec<User>,
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_is_absent_from_serialization_and_debug() {
        let user = User {
            id: Uuid::nil(),
            name: "Example".to_owned(),
            email: "example@example.com".to_owned(),
            password_hash: "sensitive-password-hash".to_owned(),
            role: "user".to_owned(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let serialized = serde_json::to_value(&user).unwrap();
        assert!(serialized.get("password_hash").is_none());
        let debug = format!("{user:?}");
        assert!(!debug.contains("password_hash"));
        assert!(!debug.contains("sensitive-password-hash"));

        let new_user = NewUser {
            id: user.id,
            name: user.name,
            email: user.email,
            password_hash: user.password_hash,
        };
        let debug = format!("{new_user:?}");
        assert!(!debug.contains("password_hash"));
        assert!(!debug.contains("sensitive-password-hash"));
    }
}
