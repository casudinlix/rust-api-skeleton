use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use crate::{
    config::JwtConfig,
    domain::models::user::{NewUser, User, UserCursor, UserPage, UserUpdate},
    errors::{AppError, AppResult},
    repositories::UserRepository,
};

#[derive(Default)]
pub(crate) struct MemoryUsers {
    users: Mutex<HashMap<Uuid, User>>,
    pub list_calls: Mutex<Vec<u32>>,
    pub id_lookups: AtomicUsize,
    pub created_jobs: AtomicUsize,
}

impl MemoryUsers {
    pub fn insert(&self, user: User) {
        self.users.lock().unwrap().insert(user.id, user);
    }

    pub fn set_role(&self, id: Uuid, role: &str) {
        self.users.lock().unwrap().get_mut(&id).unwrap().role = role.to_owned();
    }
}

#[async_trait]
impl UserRepository for MemoryUsers {
    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<User>> {
        self.id_lookups.fetch_add(1, Ordering::Relaxed);
        Ok(self.users.lock().unwrap().get(&id).cloned())
    }

    async fn find_by_email(&self, email: &str) -> AppResult<Option<User>> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .values()
            .find(|user| user.email == email)
            .cloned())
    }

    async fn create_with_welcome_job(&self, input: NewUser) -> AppResult<User> {
        let mut users = self.users.lock().unwrap();
        if users
            .values()
            .any(|user| user.email == input.email || user.id == input.id)
        {
            return Err(AppError::Conflict("user already exists".into()));
        }
        let now = Utc::now();
        let user = User {
            id: input.id,
            name: input.name,
            email: input.email,
            password_hash: input.password_hash,
            role: "user".into(),
            created_at: now,
            updated_at: now,
        };
        users.insert(user.id, user.clone());
        self.created_jobs.fetch_add(1, Ordering::Relaxed);
        Ok(user)
    }

    async fn list(&self, cursor: Option<UserCursor>, limit: u32) -> AppResult<UserPage> {
        self.list_calls.lock().unwrap().push(limit);
        let mut users: Vec<User> = self
            .users
            .lock()
            .unwrap()
            .values()
            .filter(|user| {
                cursor.as_ref().is_none_or(|cursor| {
                    (user.created_at, user.id) < (cursor.created_at, cursor.id)
                })
            })
            .cloned()
            .collect();
        users.sort_by_key(|user| std::cmp::Reverse((user.created_at, user.id)));
        users.truncate(limit as usize);
        Ok(UserPage {
            users,
            next_cursor: None,
        })
    }

    async fn update(&self, id: Uuid, input: UserUpdate) -> AppResult<User> {
        let mut users = self.users.lock().unwrap();
        let user = users
            .get_mut(&id)
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;
        user.name = input.name;
        user.email = input.email;
        user.updated_at = Utc::now();
        Ok(user.clone())
    }

    async fn delete(&self, id: Uuid) -> AppResult<bool> {
        Ok(self.users.lock().unwrap().remove(&id).is_some())
    }
}

pub(crate) fn fixture_user() -> User {
    let now = Utc::now();
    User {
        id: Uuid::now_v7(),
        name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        password_hash: "not-used-in-this-test".into(),
        role: "user".into(),
        created_at: now,
        updated_at: now,
    }
}

pub(crate) fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: "test-only-secret-with-at-least-32-bytes".into(),
        issuer: "rust-api-skeleton-tests".into(),
        audience: "rust-api-clients".into(),
        ttl_secs: 3600,
    }
}
