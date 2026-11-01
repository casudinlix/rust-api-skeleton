use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::json;
use sqlx::{MySqlPool, Row, mysql::MySqlRow, types::Json};
use uuid::Uuid;

use crate::{
    database::connection::mysql_text,
    domain::models::user::{NewUser, User, UserCursor, UserPage, UserUpdate},
    errors::{AppError, AppResult},
    repositories::user_repository::{
        UserRepository, map_user_write_error, normalize_email, page_from_users, page_limit,
    },
};

#[derive(Clone)]
pub struct MysqlUserRepository {
    pool: MySqlPool,
}

impl MysqlUserRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

fn user_from_row(row: MySqlRow) -> Result<User, sqlx::Error> {
    let id = mysql_text(&row, "id")?;
    let created_at: NaiveDateTime = row.try_get("created_at")?;
    let updated_at: NaiveDateTime = row.try_get("updated_at")?;
    Ok(User {
        id: Uuid::parse_str(&id).map_err(|source| sqlx::Error::ColumnDecode {
            index: "id".to_owned(),
            source: Box::new(source),
        })?,
        name: mysql_text(&row, "name")?,
        email: mysql_text(&row, "email")?,
        password_hash: mysql_text(&row, "password_hash")?,
        role: mysql_text(&row, "role")?,
        // MySQL DATETIME carries no timezone. Every stored value and session is UTC.
        created_at: DateTime::from_naive_utc_and_offset(created_at, Utc),
        updated_at: DateTime::from_naive_utc_and_offset(updated_at, Utc),
    })
}

#[async_trait]
impl UserRepository for MysqlUserRepository {
    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<User>> {
        sqlx::query(
            "SELECT id, name, email, password_hash, role, created_at, updated_at \
             FROM users WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .map(user_from_row)
        .transpose()
        .map_err(Into::into)
    }

    async fn find_by_email(&self, email: &str) -> AppResult<Option<User>> {
        sqlx::query(
            "SELECT id, name, email, password_hash, role, created_at, updated_at \
             FROM users WHERE email = ?",
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
        sqlx::query("INSERT INTO users (id, name, email, password_hash) VALUES (?, ?, ?, ?)")
            .bind(input.id.to_string())
            .bind(input.name)
            .bind(normalize_email(&input.email))
            .bind(input.password_hash)
            .execute(&mut *transaction)
            .await
            .map_err(map_user_write_error)?;

        sqlx::query("INSERT INTO jobs (id, kind, payload, deduplication_key) VALUES (?, ?, ?, ?)")
            .bind(Uuid::new_v4().to_string())
            .bind("send_welcome_email")
            .bind(Json(json!({ "user_id": input.id })))
            .bind(format!("welcome:{}", input.id))
            .execute(&mut *transaction)
            .await?;

        let row = sqlx::query(
            "SELECT id, name, email, password_hash, role, created_at, updated_at \
             FROM users WHERE id = ?",
        )
        .bind(input.id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        let user = user_from_row(row)?;
        transaction.commit().await?;
        Ok(user)
    }

    async fn list(&self, cursor: Option<UserCursor>, limit: u32) -> AppResult<UserPage> {
        let fetch_limit = page_limit(limit)?;
        let rows = match cursor {
            Some(cursor) => {
                sqlx::query(
                    "SELECT id, name, email, password_hash, role, created_at, updated_at \
                     FROM users WHERE created_at < ? OR (created_at = ? AND id < ?) \
                     ORDER BY created_at DESC, id DESC LIMIT ?",
                )
                .bind(cursor.created_at.naive_utc())
                .bind(cursor.created_at.naive_utc())
                .bind(cursor.id.to_string())
                .bind(fetch_limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT id, name, email, password_hash, role, created_at, updated_at \
                     FROM users ORDER BY created_at DESC, id DESC LIMIT ?",
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
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE users SET name = ?, email = ?, updated_at = UTC_TIMESTAMP(6) WHERE id = ?",
        )
        .bind(input.name)
        .bind(normalize_email(&input.email))
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(map_user_write_error)?;
        // A transaction keeps this read consistent with the row we just updated.
        // Do not infer absence from affected rows: an unchanged value can report 0.
        let row = sqlx::query(
            "SELECT id, name, email, password_hash, role, created_at, updated_at \
             FROM users WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_owned()))?;
        let user = user_from_row(row)?;
        transaction.commit().await?;
        Ok(user)
    }

    async fn delete(&self, id: Uuid) -> AppResult<bool> {
        let result = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
