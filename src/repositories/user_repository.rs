use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use uuid::Uuid;

use crate::{
    domain::models::user::{NewUser, User, UserContact, UserCursor, UserPage, UserUpdate},
    errors::{AppError, AppResult},
};

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<User>>;
    async fn find_contact_by_id(&self, id: Uuid) -> AppResult<Option<UserContact>>;
    async fn find_by_email(&self, email: &str) -> AppResult<Option<User>>;
    async fn create_with_welcome_job(&self, user: NewUser) -> AppResult<User>;
    async fn list(&self, cursor: Option<UserCursor>, limit: u32) -> AppResult<UserPage>;
    async fn update(&self, id: Uuid, input: UserUpdate) -> AppResult<User>;
    async fn delete(&self, id: Uuid) -> AppResult<bool>;
}

pub(crate) fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

pub(crate) fn map_user_write_error(error: sqlx::Error) -> AppError {
    if error
        .as_database_error()
        .is_some_and(|database_error| database_error.is_unique_violation())
    {
        AppError::Conflict("A user with that email or ID already exists".to_owned())
    } else {
        error.into()
    }
}

pub(crate) fn page_limit(limit: u32) -> AppResult<i64> {
    if !(1..=100).contains(&limit) {
        return Err(AppError::BadRequest(
            "limit must be between 1 and 100".to_owned(),
        ));
    }
    Ok(i64::from(limit) + 1)
}

pub(crate) fn page_from_users(mut users: Vec<User>, limit: u32) -> AppResult<UserPage> {
    let has_more = users.len() > limit as usize;
    users.truncate(limit as usize);
    let next_cursor = if has_more {
        users
            .last()
            .map(|user| {
                let cursor = UserCursor {
                    created_at: user.created_at,
                    id: user.id,
                };
                serde_json::to_vec(&cursor).map(|json| URL_SAFE_NO_PAD.encode(json))
            })
            .transpose()
            .map_err(anyhow::Error::from)?
    } else {
        None
    };
    Ok(UserPage { users, next_cursor })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn user(number: u128) -> User {
        User {
            id: Uuid::from_u128(number),
            name: "Example".to_owned(),
            email: format!("user-{number}@example.com"),
            password_hash: "unused".to_owned(),
            role: "user".to_owned(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn cursor_uses_last_returned_user_and_omits_lookahead() {
        let page = page_from_users(vec![user(3), user(2), user(1)], 2).unwrap();
        assert_eq!(page.users.len(), 2);
        let encoded = page.next_cursor.unwrap();
        assert!(!encoded.contains('='));
        let cursor: UserCursor =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
        assert_eq!(cursor.id, Uuid::from_u128(2));
        assert_eq!(cursor.created_at, page.users[1].created_at);
    }

    #[test]
    fn final_page_and_empty_page_have_no_cursor() {
        assert!(
            page_from_users(vec![user(2), user(1)], 2)
                .unwrap()
                .next_cursor
                .is_none()
        );
        assert!(
            page_from_users(Vec::new(), 2)
                .unwrap()
                .next_cursor
                .is_none()
        );
    }

    #[test]
    fn rejects_unbounded_or_zero_page_sizes() {
        assert!(page_limit(0).is_err());
        assert!(page_limit(101).is_err());
        assert!(page_limit(u32::MAX).is_err());
        assert_eq!(page_limit(100).unwrap(), 101);
    }

    #[test]
    fn normalization_is_stable_and_removes_surrounding_space() {
        assert_eq!(
            normalize_email("  Person@Example.COM  "),
            "person@example.com"
        );
        assert_eq!(
            normalize_email(&normalize_email("Person@Example.COM")),
            "person@example.com"
        );
    }
}
