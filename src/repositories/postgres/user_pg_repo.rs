use async_trait::async_trait;
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgRow, types::Json};
use uuid::Uuid;

use crate::{
    config::QueueDriver,
    domain::models::user::{NewUser, User, UserContact, UserCursor, UserPage, UserUpdate},
    errors::{AppError, AppResult},
    repositories::user_repository::{
        UserRepository, map_user_write_error, normalize_email, page_from_users, page_limit,
    },
};

#[derive(Clone)]
pub struct PostgresUserRepository {
    pool: PgPool,
    queue_driver: QueueDriver,
}

impl PostgresUserRepository {
    pub fn new(pool: PgPool) -> Self {
        Self::with_queue_driver(pool, QueueDriver::Database)
    }

    pub fn with_queue_driver(pool: PgPool, queue_driver: QueueDriver) -> Self {
        Self { pool, queue_driver }
    }
}

fn user_from_row(row: PgRow) -> Result<User, sqlx::Error> {
    Ok(User {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        email: row.try_get("email")?,
        password_hash: row.try_get("password_hash")?,
        role: row.try_get("role")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[async_trait]
impl UserRepository for PostgresUserRepository {
    async fn find_contact_by_id(&self, id: Uuid) -> AppResult<Option<UserContact>> {
        let row = sqlx::query("SELECT id AS user_id, name, email FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            Ok(UserContact {
                user_id: row.try_get("user_id")?,
                name: row.try_get("name")?,
                email: row.try_get("email")?,
            })
        })
        .transpose()
    }

    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<User>> {
        sqlx::query(
            "SELECT id, name, email, password_hash, role, created_at, updated_at \
             FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .map(user_from_row)
        .transpose()
        .map_err(Into::into)
    }

    async fn find_by_email(&self, email: &str) -> AppResult<Option<User>> {
        sqlx::query(
            "SELECT id, name, email, password_hash, role, created_at, updated_at \
             FROM users WHERE email = $1",
        )
        .bind(normalize_email(email))
        .fetch_optional(&self.pool)
        .await?
        .map(user_from_row)
        .transpose()
        .map_err(Into::into)
    }

    async fn create_with_welcome_job(&self, input: NewUser) -> AppResult<User> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "INSERT INTO users (id, name, email, password_hash) VALUES ($1, $2, $3, $4) \
             RETURNING id, name, email, password_hash, role, created_at, updated_at",
        )
        .bind(input.id)
        .bind(input.name)
        .bind(normalize_email(&input.email))
        .bind(input.password_hash)
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_user_write_error)?;
        let user = user_from_row(row)?;

        let payload = json!({ "user_id": user.id });
        let key = format!("welcome:{}", user.id);
        if self.queue_driver == QueueDriver::Redis {
            crate::jobs::outbox::insert_postgres(
                &mut transaction,
                "send_welcome_email",
                &payload,
                Some(&key),
            )
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO jobs (id, kind, payload, deduplication_key) \
             VALUES ($1, $2, $3, $4)",
            )
            .bind(Uuid::now_v7())
            .bind("send_welcome_email")
            .bind(Json(payload))
            .bind(key)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(user)
    }

    async fn list(&self, cursor: Option<UserCursor>, limit: u32) -> AppResult<UserPage> {
        let fetch_limit = page_limit(limit)?;
        let rows = match cursor {
            Some(cursor) => {
                sqlx::query(
                    "SELECT id, name, email, password_hash, role, created_at, updated_at \
                     FROM users WHERE (created_at, id) < ($1, $2) \
                     ORDER BY created_at DESC, id DESC LIMIT $3",
                )
                .bind(cursor.created_at)
                .bind(cursor.id)
                .bind(fetch_limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT id, name, email, password_hash, role, created_at, updated_at \
                     FROM users ORDER BY created_at DESC, id DESC LIMIT $1",
                )
                .bind(fetch_limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        let users = rows
            .into_iter()
            .map(user_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        page_from_users(users, limit)
    }

    async fn update(&self, id: Uuid, input: UserUpdate) -> AppResult<User> {
        let row = sqlx::query(
            "UPDATE users SET name = $1, email = $2, updated_at = CURRENT_TIMESTAMP \
             WHERE id = $3 \
             RETURNING id, name, email, password_hash, role, created_at, updated_at",
        )
        .bind(input.name)
        .bind(normalize_email(&input.email))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_user_write_error)?
        .ok_or_else(|| AppError::NotFound("User not found".to_owned()))?;
        Ok(user_from_row(row)?)
    }

    async fn delete(&self, id: Uuid) -> AppResult<bool> {
        let result = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
