use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    config::JwtConfig,
    domain::models::user::{
        NewUser, User, UserContact, UserCredentials, UserCursor, UserPage, UserUpdate,
    },
    errors::{AppError, AppResult},
    repositories::{
        UserRepository,
        user_repository::{page_from_users, page_limit, validate_new_user_id},
    },
};

#[derive(Default)]
pub(crate) struct MemoryUsers {
    users: Mutex<HashMap<Uuid, UserCredentials>>,
    pub list_calls: Mutex<Vec<u32>>,
    pub id_lookups: AtomicUsize,
    pub created_jobs: AtomicUsize,
}

impl MemoryUsers {
    pub fn insert(&self, user: User) {
        self.insert_with_password_hash(user, "not-used-in-this-test".into());
    }

    pub fn insert_with_password_hash(&self, user: User, password_hash: String) {
        self.users.lock().unwrap().insert(
            user.user_id,
            UserCredentials {
                user,
                password_hash,
            },
        );
    }

    pub fn set_role(&self, user_id: Uuid, role: &str) {
        self.users
            .lock()
            .unwrap()
            .get_mut(&user_id)
            .unwrap()
            .user
            .role = role.to_owned();
    }
}

#[async_trait]
impl UserRepository for MemoryUsers {
    async fn find_contact_by_id(&self, user_id: Uuid) -> AppResult<Option<UserContact>> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .get(&user_id)
            .map(|credentials| UserContact {
                user_id: credentials.user.user_id,
                name: credentials.user.name.clone(),
                email: credentials.user.email.clone(),
            }))
    }

    async fn find_by_id(&self, user_id: Uuid) -> AppResult<Option<User>> {
        self.id_lookups.fetch_add(1, Ordering::Relaxed);
        Ok(self
            .users
            .lock()
            .unwrap()
            .get(&user_id)
            .map(|credentials| credentials.user.clone()))
    }

    async fn find_credentials_by_email(&self, email: &str) -> AppResult<Option<UserCredentials>> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .values()
            .find(|credentials| credentials.user.email == email)
            .cloned())
    }

    async fn create_with_welcome_job(&self, input: NewUser) -> AppResult<User> {
        validate_new_user_id(input.user_id)?;
        let mut users = self.users.lock().unwrap();
        if users.values().any(|credentials| {
            credentials.user.email == input.email || credentials.user.user_id == input.user_id
        }) {
            return Err(AppError::Conflict("user already exists".into()));
        }
        let now = database_timestamp();
        let user = User {
            user_id: input.user_id,
            name: input.name,
            email: input.email,
            role: "user".into(),
            created_at: now,
            updated_at: now,
        };
        users.insert(
            user.user_id,
            UserCredentials {
                user: user.clone(),
                password_hash: input.password_hash,
            },
        );
        self.created_jobs.fetch_add(1, Ordering::Relaxed);
        Ok(user)
    }

    async fn list(&self, cursor: Option<UserCursor>, limit: u32) -> AppResult<UserPage> {
        let fetch_limit = page_limit(limit)?;
        self.list_calls.lock().unwrap().push(limit);
        let mut users: Vec<User> = self
            .users
            .lock()
            .unwrap()
            .values()
            .map(|credentials| &credentials.user)
            .filter(|user| {
                cursor.as_ref().is_none_or(|cursor| {
                    (user.created_at, user.user_id) < (cursor.created_at, cursor.user_id)
                })
            })
            .cloned()
            .collect();
        users.sort_by_key(|user| std::cmp::Reverse((user.created_at, user.user_id)));
        users.truncate(fetch_limit as usize);
        Ok(page_from_users(users, limit))
    }

    async fn update(&self, user_id: Uuid, input: UserUpdate) -> AppResult<User> {
        let mut users = self.users.lock().unwrap();
        let credentials = users
            .get_mut(&user_id)
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;
        let user = &mut credentials.user;
        user.name = input.name;
        user.email = input.email;
        user.updated_at = database_timestamp();
        Ok(user.clone())
    }

    async fn delete(&self, user_id: Uuid) -> AppResult<bool> {
        Ok(self.users.lock().unwrap().remove(&user_id).is_some())
    }
}

pub(crate) fn fixture_user() -> User {
    let now = database_timestamp();
    User {
        user_id: Uuid::now_v7(),
        name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        role: "user".into(),
        created_at: now,
        updated_at: now,
    }
}

fn database_timestamp() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
}

pub(crate) fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: "test-only-secret-with-at-least-32-bytes".into(),
        issuer: "rust-api-skeleton-tests".into(),
        audience: "rust-api-clients".into(),
        ttl_secs: 3600,
    }
}
