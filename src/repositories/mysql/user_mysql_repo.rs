use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::json;
use sqlx::{MySqlPool, Row, mysql::MySqlRow, types::Json};
use uuid::Uuid;

use crate::{
    config::QueueDriver,
    database::connection::mysql_text,
    domain::models::user::{
        NewUser, User, UserContact, UserCredentials, UserCursor, UserPage, UserUpdate,
    },
    errors::{AppError, AppResult},
    repositories::user_repository::{
        UserRepository, map_user_write_error, normalize_email, page_from_users, page_limit,
        validate_new_user_id,
    },
};

#[derive(Clone)]
pub struct MysqlUserRepository {
    pool: MySqlPool,
    queue_driver: QueueDriver,
}

impl MysqlUserRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self::with_queue_driver(pool, QueueDriver::Database)
    }

    pub fn with_queue_driver(pool: MySqlPool, queue_driver: QueueDriver) -> Self {
        Self { pool, queue_driver }
    }
}

fn user_from_row(row: &MySqlRow) -> Result<User, sqlx::Error> {
    let user_id = mysql_text(row, "user_id")?;
    let created_at: NaiveDateTime = row.try_get("created_at")?;
    let updated_at: NaiveDateTime = row.try_get("updated_at")?;
    Ok(User {
        user_id: Uuid::parse_str(&user_id).map_err(|source| sqlx::Error::ColumnDecode {
            index: "user_id".to_owned(),
            source: Box::new(source),
        })?,
        name: mysql_text(row, "name")?,
        email: mysql_text(row, "email")?,
        role: mysql_text(row, "role")?,
        // MySQL DATETIME carries no timezone. Every stored value and session is UTC.
        created_at: DateTime::from_naive_utc_and_offset(created_at, Utc),
        updated_at: DateTime::from_naive_utc_and_offset(updated_at, Utc),
    })
}

fn credentials_from_row(row: &MySqlRow) -> Result<UserCredentials, sqlx::Error> {
    Ok(UserCredentials {
        user: user_from_row(row)?,
        password_hash: mysql_text(row, "password_hash")?,
    })
}

#[async_trait]
impl UserRepository for MysqlUserRepository {
    async fn find_contact_by_id(&self, user_id: Uuid) -> AppResult<Option<UserContact>> {
        let row = sqlx::query("SELECT user_id, name, email FROM users WHERE user_id = ?")
            .bind(user_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            let text = mysql_text(&row, "user_id")?;
            let user_id = Uuid::parse_str(&text)
                .map_err(|_| anyhow::anyhow!("invalid stored user identifier"))?;
            Ok(UserContact {
                user_id,
                name: mysql_text(&row, "name")?,
                email: mysql_text(&row, "email")?,
            })
        })
        .transpose()
    }

    async fn find_by_id(&self, user_id: Uuid) -> AppResult<Option<User>> {
        sqlx::query(
            "SELECT user_id, name, email, role, created_at, updated_at \
             FROM users WHERE user_id = ?",
        )
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .map(|row| user_from_row(&row))
        .transpose()
        .map_err(Into::into)
    }

    async fn find_credentials_by_email(&self, email: &str) -> AppResult<Option<UserCredentials>> {
        sqlx::query(
            "SELECT user_id, name, email, password_hash, role, created_at, updated_at \
             FROM users WHERE email = ?",
        )
        .bind(normalize_email(email))
        .fetch_optional(&self.pool)
        .await?
        .map(|row| credentials_from_row(&row))
        .transpose()
        .map_err(Into::into)
    }

    async fn create_with_welcome_job(&self, input: NewUser) -> AppResult<User> {
        validate_new_user_id(input.user_id)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query("INSERT INTO users (user_id, name, email, password_hash) VALUES (?, ?, ?, ?)")
            .bind(input.user_id.to_string())
            .bind(input.name)
            .bind(normalize_email(&input.email))
            .bind(input.password_hash)
            .execute(&mut *transaction)
            .await
            .map_err(map_user_write_error)?;

        let payload = json!({ "user_id": input.user_id });
        let key = format!("welcome:{}", input.user_id);
        if self.queue_driver == QueueDriver::Redis {
            crate::jobs::outbox::insert_mysql(
                &mut transaction,
                "send_welcome_email",
                &payload,
                Some(&key),
            )
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO jobs (job_id, kind, payload, deduplication_key) VALUES (?, ?, ?, ?)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind("send_welcome_email")
            .bind(Json(payload))
            .bind(key)
            .execute(&mut *transaction)
            .await?;
        }

        let row = sqlx::query(
            "SELECT user_id, name, email, role, created_at, updated_at \
             FROM users WHERE user_id = ?",
        )
        .bind(input.user_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        let user = user_from_row(&row)?;
        transaction.commit().await?;
        Ok(user)
    }

    async fn list(&self, cursor: Option<UserCursor>, limit: u32) -> AppResult<UserPage> {
        let fetch_limit = page_limit(limit)?;
        let rows = match cursor {
            Some(cursor) => {
                sqlx::query(
                    "SELECT user_id, name, email, role, created_at, updated_at \
                     FROM users WHERE created_at < ? OR (created_at = ? AND user_id < ?) \
                     ORDER BY created_at DESC, user_id DESC LIMIT ?",
                )
                .bind(cursor.created_at.naive_utc())
                .bind(cursor.created_at.naive_utc())
                .bind(cursor.user_id.to_string())
                .bind(fetch_limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT user_id, name, email, role, created_at, updated_at \
                     FROM users ORDER BY created_at DESC, user_id DESC LIMIT ?",
                )
                .bind(fetch_limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        let users = rows
            .into_iter()
            .map(|row| user_from_row(&row))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(page_from_users(users, limit))
    }

    async fn update(&self, user_id: Uuid, input: UserUpdate) -> AppResult<User> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE users SET name = ?, email = ?, updated_at = UTC_TIMESTAMP(6) WHERE user_id = ?",
        )
        .bind(input.name)
        .bind(normalize_email(&input.email))
        .bind(user_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(map_user_write_error)?;
        // A transaction keeps this read consistent with the row we just updated.
        // Do not infer absence from affected rows: an unchanged value can report 0.
        let row = sqlx::query(
            "SELECT user_id, name, email, role, created_at, updated_at \
             FROM users WHERE user_id = ?",
        )
        .bind(user_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_owned()))?;
        let user = user_from_row(&row)?;
        transaction.commit().await?;
        Ok(user)
    }

    async fn delete(&self, user_id: Uuid) -> AppResult<bool> {
        let result = sqlx::query("DELETE FROM users WHERE user_id = ?")
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
