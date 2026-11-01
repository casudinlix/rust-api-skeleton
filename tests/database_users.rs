//! Run explicitly with a disposable database, for example:
//! TEST_DATABASE_URL=postgres://... cargo test --test database_users postgres_users -- --ignored
//! Tests create only unique fixtures and remove only their own rows.

use std::{collections::HashSet, sync::Arc};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Timelike, Utc};
use rust_api_skeleton::{
    config::database::{DatabaseConfig, DatabaseDriver},
    database::DatabasePool,
    domain::models::user::{NewUser, User, UserCursor, UserUpdate},
    errors::AppError,
    repositories::{UserRepository, user_repository},
};
use serde_json::{Value, json};
use sqlx::{Row, types::Json};
use uuid::Uuid;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn postgres_users() {
    exercise_users(DatabaseDriver::Postgres).await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable MySQL 8 database"]
async fn mysql_users() {
    exercise_users(DatabaseDriver::Mysql).await;
}

fn new_user(id: Uuid, email: String) -> NewUser {
    NewUser {
        id,
        name: "A name with ' SQL punctuation; --".to_owned(),
        email,
        password_hash: "test-only-password-hash".to_owned(),
    }
}

async fn exercise_users(driver: DatabaseDriver) {
    let database = DatabasePool::connect(&DatabaseConfig {
        driver,
        url: std::env::var("TEST_DATABASE_URL").expect("set TEST_DATABASE_URL explicitly"),
        max_connections: 5,
        min_connections: 2,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
    })
    .await
    .expect("connect to test database");
    database.migrate().await.unwrap();
    database.migrate().await.unwrap(); // Migration application must be idempotent.
    database.ping().await.unwrap();
    database.ready().await.unwrap();
    assert_utc_sessions(&database).await;
    let repository = user_repository(&database);

    let id = Uuid::new_v4();
    let email = format!("test-{id}@example.com");
    let created = repository
        .create_with_welcome_job(new_user(id, format!("  {}  ", email.to_uppercase())))
        .await
        .unwrap();
    assert_eq!(created.email, email);
    assert_eq!(created.role, "user");
    assert_eq!(created.password_hash, "test-only-password-hash");
    assert_eq!(created.created_at.timestamp_subsec_nanos() % 1000, 0);
    assert!((Utc::now() - created.created_at).num_seconds().abs() < 30);
    assert_eq!(repository.find_by_id(id).await.unwrap().unwrap().id, id);
    assert_eq!(
        repository
            .find_by_email(&email.to_uppercase())
            .await
            .unwrap()
            .unwrap()
            .id,
        id
    );
    assert_welcome_job(&database, id).await;

    let duplicate_id = Uuid::new_v4();
    assert!(matches!(
        repository
            .create_with_welcome_job(new_user(duplicate_id, email.clone()))
            .await,
        Err(AppError::Conflict(_))
    ));
    assert!(repository.find_by_id(duplicate_id).await.unwrap().is_none());
    assert_eq!(welcome_count(&database, duplicate_id).await, 0);

    let updated_email = format!("updated-{id}@example.com");
    let updated = repository
        .update(
            id,
            UserUpdate {
                name: "Changed ' name".to_owned(),
                email: updated_email.to_uppercase(),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "Changed ' name");
    assert_eq!(updated.email, updated_email);
    assert_eq!(updated.role, "user");
    assert_eq!(updated.password_hash, created.password_hash);
    assert_eq!(updated.created_at, created.created_at);
    assert!(updated.updated_at >= created.updated_at);
    assert!(repository.find_by_email(&email).await.unwrap().is_none());
    let unchanged = repository
        .update(
            id,
            UserUpdate {
                name: updated.name.clone(),
                email: updated.email.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(unchanged.id, id);
    assert!(matches!(
        repository
            .update(
                Uuid::new_v4(),
                UserUpdate {
                    name: "Absent".to_owned(),
                    email: email.clone()
                }
            )
            .await,
        Err(AppError::NotFound(_))
    ));

    assert_transaction_rollback(&database, repository.clone()).await;
    assert_concurrent_duplicate_registration(&database, repository.clone()).await;
    assert_tied_timestamp_pagination(&database, repository.clone()).await;

    assert!(repository.delete(id).await.unwrap());
    assert!(!repository.delete(id).await.unwrap());
    assert!(repository.find_by_id(id).await.unwrap().is_none());
    delete_welcome_job(&database, id).await;
    database.close().await;
    assert!(database.ping().await.is_err());
}

async fn assert_transaction_rollback(database: &DatabasePool, repository: Arc<dyn UserRepository>) {
    let id = Uuid::new_v4();
    let deduplication_key = format!("welcome:{id}");
    // A job collision occurs after the user INSERT, exercising actual rollback.
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query(
                "INSERT INTO jobs (id, kind, payload, deduplication_key) VALUES ($1, $2, $3, $4)",
            )
            .bind(Uuid::new_v4())
            .bind("send_welcome_email")
            .bind(Json(json!({"user_id": id})))
            .bind(&deduplication_key)
            .execute(pool)
            .await
            .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query(
                "INSERT INTO jobs (id, kind, payload, deduplication_key) VALUES (?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind("send_welcome_email")
            .bind(Json(json!({"user_id": id})))
            .bind(&deduplication_key)
            .execute(pool)
            .await
            .unwrap();
        }
    }
    assert!(
        repository
            .create_with_welcome_job(new_user(id, format!("rollback-{id}@example.com")))
            .await
            .is_err()
    );
    assert!(repository.find_by_id(id).await.unwrap().is_none());
    assert_eq!(welcome_count(database, id).await, 1);
    delete_welcome_job(database, id).await;
}

async fn assert_concurrent_duplicate_registration(
    database: &DatabasePool,
    repository: Arc<dyn UserRepository>,
) {
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();
    let email = format!("race-{id_a}@example.com");
    let (first, second) = tokio::join!(
        repository.create_with_welcome_job(new_user(id_a, email.clone())),
        repository.create_with_welcome_job(new_user(id_b, email.to_uppercase()))
    );
    let winner: User = match (first, second) {
        (Ok(user), Err(AppError::Conflict(_))) | (Err(AppError::Conflict(_)), Ok(user)) => user,
        outcomes => panic!("expected one success and one conflict, got {outcomes:?}"),
    };
    assert_eq!(
        welcome_count(database, id_a).await + welcome_count(database, id_b).await,
        1
    );
    assert_welcome_job(database, winner.id).await;
    repository.delete(winner.id).await.unwrap();
    delete_welcome_job(database, winner.id).await;
}

async fn assert_tied_timestamp_pagination(
    database: &DatabasePool,
    repository: Arc<dyn UserRepository>,
) {
    let now = Utc::now();
    let timestamp = (now + Duration::days(365 * 200))
        .with_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap();
    let mut ids = vec![Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    for id in &ids {
        repository
            .create_with_welcome_job(new_user(*id, format!("page-{id}@example.com")))
            .await
            .unwrap();
        set_created_at(database, *id, timestamp).await;
    }
    ids.sort_unstable_by(|left, right| right.cmp(left));
    let start = UserCursor {
        created_at: timestamp,
        id: Uuid::from_u128(u128::MAX),
    };
    let first = repository.list(Some(start), 2).await.unwrap();
    assert_eq!(
        first.users.iter().map(|user| user.id).collect::<Vec<_>>(),
        ids[..2]
    );
    let cursor: UserCursor = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(first.next_cursor.as_ref().expect("lookahead cursor"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cursor.id, ids[1]);
    assert_eq!(cursor.created_at, timestamp);

    // Deleting the cursor row must not skip the remaining tied-timestamp row.
    repository.delete(ids[1]).await.unwrap();
    let second = repository.list(Some(cursor), 1).await.unwrap();
    assert_eq!(second.users[0].id, ids[2]);
    let observed: HashSet<_> = first
        .users
        .iter()
        .chain(&second.users)
        .map(|user| user.id)
        .collect();
    assert_eq!(observed.len(), 3);
    assert!(repository.list(None, 0).await.is_err());
    assert!(repository.list(None, 101).await.is_err());

    for id in ids {
        repository.delete(id).await.unwrap();
        delete_welcome_job(database, id).await;
    }
}

async fn assert_utc_sessions(database: &DatabasePool) {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut first = pool.acquire().await.unwrap();
            let mut second = pool.acquire().await.unwrap();
            for connection in [&mut first, &mut second] {
                let timezone: String = sqlx::query_scalar("SHOW TIME ZONE")
                    .fetch_one(&mut **connection)
                    .await
                    .unwrap();
                assert_eq!(timezone, "UTC");
            }
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut first = pool.acquire().await.unwrap();
            let mut second = pool.acquire().await.unwrap();
            for connection in [&mut first, &mut second] {
                let timezone: String = sqlx::query_scalar("SELECT @@session.time_zone")
                    .fetch_one(&mut **connection)
                    .await
                    .unwrap();
                assert_eq!(timezone, "+00:00");
            }
        }
    }
}

async fn assert_welcome_job(database: &DatabasePool, id: Uuid) {
    let deduplication_key = format!("welcome:{id}");
    let (kind, payload, status): (String, Json<Value>, String) = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let row =
                sqlx::query("SELECT kind, payload, status FROM jobs WHERE deduplication_key = $1")
                    .bind(deduplication_key)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            (row.get("kind"), row.get("payload"), row.get("status"))
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let row =
                sqlx::query("SELECT kind, payload, status FROM jobs WHERE deduplication_key = ?")
                    .bind(deduplication_key)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            let kind: &[u8] = row.get("kind");
            let status: &[u8] = row.get("status");
            (
                std::str::from_utf8(kind).unwrap().to_owned(),
                row.get("payload"),
                std::str::from_utf8(status).unwrap().to_owned(),
            )
        }
    };
    assert_eq!(kind, "send_welcome_email");
    assert_eq!(payload.0, json!({ "user_id": id }));
    assert_eq!(status, "pending");
}

async fn welcome_count(database: &DatabasePool, id: Uuid) -> i64 {
    let deduplication_key = format!("welcome:{id}");
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE deduplication_key = $1")
                .bind(deduplication_key)
                .fetch_one(pool)
                .await
                .unwrap()
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE deduplication_key = ?")
                .bind(deduplication_key)
                .fetch_one(pool)
                .await
                .unwrap()
        }
    }
}

async fn delete_welcome_job(database: &DatabasePool, id: Uuid) {
    let deduplication_key = format!("welcome:{id}");
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("DELETE FROM jobs WHERE deduplication_key = $1")
                .bind(deduplication_key)
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("DELETE FROM jobs WHERE deduplication_key = ?")
                .bind(deduplication_key)
                .execute(pool)
                .await
                .unwrap();
        }
    }
}

async fn set_created_at(database: &DatabasePool, id: Uuid, created_at: DateTime<Utc>) {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE users SET created_at = $1 WHERE id = $2")
                .bind(created_at)
                .bind(id)
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE users SET created_at = ? WHERE id = ?")
                .bind(created_at.naive_utc())
                .bind(id.to_string())
                .execute(pool)
                .await
                .unwrap();
        }
    }
}
