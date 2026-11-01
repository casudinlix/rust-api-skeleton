use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Datelike;
use serde::{Deserialize, Serialize};
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
const MAX_CURSOR_BYTES: usize = 384;
const CURSOR_VERSION: u8 = 1;
const CURSOR_RESOURCE: &str = "users";
const CURSOR_SORT: &str = "created_at_desc_user_id_desc";
// All users are visible only after the trusted AdminUser guard succeeds. This
// scope is never taken from the cursor to grant access or construct SQL.
const CURSOR_SCOPE: &str = "admin:all";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UserCursorEnvelope {
    version: u8,
    resource: String,
    sort: String,
    scope: String,
    position: UserCursor,
}

pub struct UserService {
    repository: Arc<dyn UserRepository>,
}

impl UserService {
    pub fn new(repository: Arc<dyn UserRepository>) -> Self {
        Self { repository }
    }

    pub async fn get(&self, user_id: Uuid) -> AppResult<UserResponse> {
        self.repository
            .find_by_id(user_id)
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
            next_cursor: page.next_cursor.map(encode_cursor).transpose()?,
        })
    }

    pub async fn update(&self, user_id: Uuid, input: UpdateUserRequest) -> AppResult<UserResponse> {
        let input = normalize_and_validate(input)?;
        self.repository
            .update(
                user_id,
                UserUpdate {
                    name: input.name,
                    email: input.email,
                },
            )
            .await
            .map(UserResponse::from)
    }

    pub async fn delete(&self, user_id: Uuid) -> AppResult<()> {
        if self.repository.delete(user_id).await? {
            Ok(())
        } else {
            Err(AppError::NotFound("user not found".into()))
        }
    }
}

fn encode_cursor(position: UserCursor) -> AppResult<String> {
    let envelope = UserCursorEnvelope {
        version: CURSOR_VERSION,
        resource: CURSOR_RESOURCE.to_owned(),
        sort: CURSOR_SORT.to_owned(),
        scope: CURSOR_SCOPE.to_owned(),
        position,
    };
    let bytes = serde_json::to_vec(&envelope).map_err(anyhow::Error::from)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(encoded: &str) -> AppResult<UserCursor> {
    let invalid = || AppError::BadRequest("invalid pagination cursor".into());
    if encoded.is_empty() || encoded.len() > MAX_CURSOR_LENGTH {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
    if bytes.len() > MAX_CURSOR_BYTES {
        return Err(invalid());
    }
    let envelope: UserCursorEnvelope = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if envelope.version != CURSOR_VERSION
        || envelope.resource != CURSOR_RESOURCE
        || envelope.sort != CURSOR_SORT
        || envelope.scope != CURSOR_SCOPE
    {
        return Err(invalid());
    }
    let cursor = envelope.position;
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
    fn cursor_round_trips_versioned_context_and_rejects_malformed_input() {
        let expected = UserCursor {
            created_at: DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap(),
            user_id: Uuid::now_v7(),
        };
        let encoded = encode_cursor(expected.clone()).unwrap();
        assert!(encoded.len() <= MAX_CURSOR_LENGTH);
        assert!(!encoded.contains('='));
        let decoded = decode_cursor(&encoded).unwrap();
        assert_eq!(decoded.created_at, expected.created_at);
        assert_eq!(decoded.user_id, expected.user_id);
        for invalid in ["", "!not-base64!", "e30", "abcd=", &"a".repeat(513)] {
            assert!(matches!(
                decode_cursor(invalid),
                Err(AppError::BadRequest(_))
            ));
        }
        let envelope: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&encoded).unwrap()).unwrap();
        assert_eq!(envelope["version"], 1);
        assert_eq!(envelope["resource"], "users");
        assert_eq!(envelope["sort"], "created_at_desc_user_id_desc");
        assert_eq!(envelope["scope"], "admin:all");

        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("resource", serde_json::json!("jobs")),
            ("sort", serde_json::json!("user_id_asc")),
            ("scope", serde_json::json!("tenant:another")),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut invalid = envelope.clone();
            invalid[field] = value;
            assert!(
                decode_cursor(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invalid).unwrap()))
                    .is_err()
            );
        }
        for (field, value) in [
            ("user_id", serde_json::json!("not-a-uuid")),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut invalid = envelope.clone();
            invalid["position"][field] = value;
            assert!(
                decode_cursor(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invalid).unwrap()))
                    .is_err()
            );
        }

        // A schema/API upgrade deliberately resets the old unversioned cursor.
        let legacy =
            serde_json::json!({ "created_at": expected.created_at, "id": expected.user_id });
        assert!(
            decode_cursor(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(&legacy).unwrap())).is_err()
        );
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
                    "version": CURSOR_VERSION,
                    "resource": CURSOR_RESOURCE,
                    "sort": CURSOR_SORT,
                    "scope": CURSOR_SCOPE,
                    "position": { "created_at": timestamp, "user_id": Uuid::now_v7() },
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
    async fn service_pages_with_context_and_rejects_foreign_scope_before_querying() {
        let repository = Arc::new(MemoryUsers::default());
        for _ in 0..3 {
            let mut user = fixture_user();
            user.email = format!("{}@example.com", user.user_id);
            repository.insert(user);
        }
        let service = UserService::new(repository.clone());
        let first = service
            .list(UserListQuery {
                cursor: None,
                limit: Some(2),
            })
            .await
            .unwrap();
        assert_eq!(first.items.len(), 2);
        let encoded = first.next_cursor.unwrap();
        let next = service
            .list(UserListQuery {
                cursor: Some(encoded.clone()),
                limit: Some(2),
            })
            .await
            .unwrap();
        assert_eq!(next.items.len(), 1);
        assert!(next.next_cursor.is_none());
        assert!(
            first
                .items
                .iter()
                .all(|user| user.user_id != next.items[0].user_id)
        );

        let mut envelope: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
        envelope["scope"] = serde_json::json!("tenant:other");
        let invalid = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).unwrap());
        assert!(matches!(
            service
                .list(UserListQuery {
                    cursor: Some(invalid),
                    limit: Some(2)
                })
                .await,
            Err(AppError::BadRequest(_))
        ));
        assert_eq!(*repository.list_calls.lock().unwrap(), vec![2, 2]);
    }

    #[tokio::test]
    async fn service_normalizes_updates_and_reports_missing_users() {
        let repository = Arc::new(MemoryUsers::default());
        let user = fixture_user();
        let user_id = user.user_id;
        repository.insert(user);
        let service = UserService::new(repository);
        let updated = service
            .update(
                user_id,
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
        service.delete(user_id).await.unwrap();
        assert!(matches!(
            service.get(user_id).await,
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            service.delete(user_id).await,
            Err(AppError::NotFound(_))
        ));
    }
}
