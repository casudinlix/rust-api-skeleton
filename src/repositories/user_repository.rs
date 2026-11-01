use async_trait::async_trait;
use uuid::{Uuid, Variant};

use crate::{
    domain::models::user::{
        NewUser, User, UserContact, UserCredentials, UserCursor, UserPage, UserUpdate,
    },
    errors::{AppError, AppResult},
};

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid) -> AppResult<Option<User>>;
    async fn find_contact_by_id(&self, user_id: Uuid) -> AppResult<Option<UserContact>>;
    async fn find_credentials_by_email(&self, email: &str) -> AppResult<Option<UserCredentials>>;
    async fn create_with_welcome_job(&self, user: NewUser) -> AppResult<User>;
    async fn list(&self, cursor: Option<UserCursor>, limit: u32) -> AppResult<UserPage>;
    async fn update(&self, user_id: Uuid, input: UserUpdate) -> AppResult<User>;
    async fn delete(&self, user_id: Uuid) -> AppResult<bool>;
}

/// Creation rejects caller-supplied legacy IDs; existing rows remain readable.
pub(crate) fn validate_new_user_id(user_id: Uuid) -> AppResult<()> {
    if user_id.get_version_num() != 7 || user_id.get_variant() != Variant::RFC4122 {
        return Err(AppError::BadRequest(
            "New user_id must be a UUIDv7".to_owned(),
        ));
    }
    Ok(())
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

pub(crate) fn page_from_users(mut users: Vec<User>, limit: u32) -> UserPage {
    let has_more = users.len() > limit as usize;
    users.truncate(limit as usize);
    let next_cursor = if has_more {
        users.last().map(|user| UserCursor {
            created_at: user.created_at,
            user_id: user.user_id,
        })
    } else {
        None
    };
    UserPage { users, next_cursor }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn user(user_id: Uuid) -> User {
        User {
            user_id,
            name: "Example".to_owned(),
            email: format!("user-{user_id}@example.com"),
            role: "user".to_owned(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn cursor_uses_last_returned_user_and_omits_lookahead() {
        let users: Vec<_> = (0..3).map(|_| user(Uuid::now_v7())).rev().collect();
        let expected_id = users[1].user_id;
        let page = page_from_users(users, 2);
        assert_eq!(page.users.len(), 2);
        let cursor = page.next_cursor.unwrap();
        assert_eq!(cursor.user_id, expected_id);
        assert_eq!(cursor.created_at, page.users[1].created_at);
    }

    #[test]
    fn final_page_and_empty_page_have_no_cursor() {
        assert!(
            page_from_users(vec![user(Uuid::now_v7()), user(Uuid::now_v7())], 2)
                .next_cursor
                .is_none()
        );
        assert!(page_from_users(Vec::new(), 2).next_cursor.is_none());
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

    #[test]
    fn new_user_ids_must_have_version_seven_and_rfc_variant() {
        validate_new_user_id(Uuid::now_v7()).unwrap();
        let mut invalid_variant = *Uuid::now_v7().as_bytes();
        invalid_variant[8] &= 0x7f;
        for user_id in [
            Uuid::nil(),
            Uuid::new_v4(),
            Uuid::from_bytes(invalid_variant),
        ] {
            assert!(matches!(
                validate_new_user_id(user_id),
                Err(AppError::BadRequest(_))
            ));
        }
    }
}
