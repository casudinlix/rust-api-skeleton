use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Datelike;
use uuid::Uuid;

use crate::{
    domain::{
        dtos::{
            UpdateUserRequest, UserListQuery, UserListResponse, UserResponse,
            normalize_and_validate,
        },
        models::user::{UserCursor, UserUpdate},
    },
    errors::{AppError, AppResult},
    repositories::UserRepository,
};

const DEFAULT_PAGE_LIMIT: u32 = 20;
const MAX_CURSOR_LENGTH: usize = 512;

pub struct UserService {
    repository: Arc<dyn UserRepository>,
}

impl UserService {
    pub fn new(repository: Arc<dyn UserRepository>) -> Self {
        Self { repository }
    }

    pub async fn get(&self, id: Uuid) -> AppResult<UserResponse> {
        self.repository
            .find_by_id(id)
            .await?
            .map(UserResponse::from)
            .ok_or_else(|| AppError::NotFound("user not found".into()))
    }

    pub async fn list(&self, query: UserListQuery) -> AppResult<UserListResponse> {
        let limit = query.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
        if !(1..=100).contains(&limit) {
            return Err(AppError::BadRequest(
                "limit must be between 1 and 100".into(),
            ));
        }
        let cursor = query.cursor.as_deref().map(decode_cursor).transpose()?;
        let page = self.repository.list(cursor, limit).await?;
        Ok(UserListResponse {
            items: page.users.into_iter().map(UserResponse::from).collect(),
            next_cursor: page.next_cursor,
        })
    }

    pub async fn update(&self, id: Uuid, input: UpdateUserRequest) -> AppResult<UserResponse> {
        let input = normalize_and_validate(input)?;
        self.repository
            .update(
                id,
                UserUpdate {
                    name: input.name,
                    email: input.email,
                },
            )
            .await
            .map(UserResponse::from)
    }

    pub async fn delete(&self, id: Uuid) -> AppResult<()> {
        if self.repository.delete(id).await? {
            Ok(())
        } else {
            Err(AppError::NotFound("user not found".into()))
        }
    }
}

fn decode_cursor(encoded: &str) -> AppResult<UserCursor> {
    let invalid = || AppError::BadRequest("invalid pagination cursor".into());
    if encoded.is_empty() || encoded.len() > MAX_CURSOR_LENGTH {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
    let cursor: UserCursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    // Both drivers persist microsecond timestamps. Reject values that MySQL's
    // DATETIME(6) cannot encode, including chrono's optional leap-second form.
    let nanos = cursor.created_at.timestamp_subsec_nanos();
    if !(1000..=9999).contains(&cursor.created_at.year())
        || nanos >= 1_000_000_000
        || !nanos.is_multiple_of(1000)
    {
        return Err(invalid());
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::services::test_support::{MemoryUsers, fixture_user};

    #[test]
    fn cursor_accepts_repository_format_and_rejects_unbounded_or_malformed_input() {
        let expected = UserCursor {
            created_at: DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap(),
            id: Uuid::now_v7(),
        };
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&expected).unwrap());
        let decoded = decode_cursor(&encoded).unwrap();
        assert_eq!(decoded.created_at, expected.created_at);
        assert_eq!(decoded.id, expected.id);
        for invalid in ["", "!not-base64!", "e30", "abcd=", &"a".repeat(513)] {
            assert!(matches!(
                decode_cursor(invalid),
                Err(AppError::BadRequest(_))
            ));
        }
        let extra = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "created_at": expected.created_at,
                "id": expected.id,
                "unexpected": true,
            }))
            .unwrap(),
        );
        assert!(decode_cursor(&extra).is_err());
        let invalid_id = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "created_at": expected.created_at,
                "id": "not-a-uuid",
            }))
            .unwrap(),
        );
        assert!(decode_cursor(&invalid_id).is_err());
    }

    #[test]
    fn cursor_rejects_values_not_representable_by_both_databases() {
        for timestamp in [
            "0999-12-31T23:59:59Z",
            "+10000-01-01T00:00:00Z",
            "2026-11-01T00:00:00.123456789Z",
            "2016-12-31T23:59:60Z",
        ] {
            let encoded = URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&serde_json::json!({
                    "created_at": timestamp,
                    "id": Uuid::now_v7(),
                }))
                .unwrap(),
            );
            assert!(
                matches!(decode_cursor(&encoded), Err(AppError::BadRequest(_))),
                "accepted {timestamp}"
            );
        }
    }

    #[tokio::test]
    async fn pagination_rejects_invalid_limits_and_applies_default() {
        let repository = Arc::new(MemoryUsers::default());
        let service = UserService::new(repository.clone());
        for limit in [0, 101, u32::MAX] {
            assert!(matches!(
                service
                    .list(UserListQuery {
                        cursor: None,
                        limit: Some(limit)
                    })
                    .await,
                Err(AppError::BadRequest(_)),
            ));
        }
        assert_eq!(repository.list_calls.lock().unwrap().len(), 0);
        service.list(UserListQuery::default()).await.unwrap();
        service
            .list(UserListQuery {
                cursor: None,
                limit: Some(100),
            })
            .await
            .unwrap();
        assert_eq!(*repository.list_calls.lock().unwrap(), vec![20, 100]);
    }

    #[tokio::test]
    async fn service_normalizes_updates_and_reports_missing_users() {
        let repository = Arc::new(MemoryUsers::default());
        let user = fixture_user();
        let id = user.id;
        repository.insert(user);
        let service = UserService::new(repository);
        let updated = service
            .update(
                id,
                UpdateUserRequest {
                    name: "  Updated Name  ".into(),
                    email: "  UPDATED@EXAMPLE.COM  ".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.email, "updated@example.com");
        assert_eq!(updated.role, "user");
        service.delete(id).await.unwrap();
        assert!(matches!(service.get(id).await, Err(AppError::NotFound(_))));
        assert!(matches!(
            service.delete(id).await,
            Err(AppError::NotFound(_))
        ));
    }
}
