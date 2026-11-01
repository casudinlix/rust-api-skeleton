//! Run explicitly with a disposable database, for example:
//! TEST_DATABASE_URL=postgres://... cargo test --test database_users postgres_users -- --ignored
//! Tests create only unique fixtures and remove only their own rows.

use std::{
    collections::HashSet,
    future::Future,
    sync::{Arc, Mutex},
};

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
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    instrument::WithSubscriber,
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};
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

fn new_user(user_id: Uuid, email: String) -> NewUser {
    NewUser {
        user_id,
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

    for invalid_user_id in [Uuid::nil(), Uuid::new_v4()] {
        assert!(matches!(
            repository
                .create_with_welcome_job(new_user(
                    invalid_user_id,
                    format!("invalid-{invalid_user_id}@example.com")
                ))
                .await,
            Err(AppError::BadRequest(_)),
        ));
        assert!(
            repository
                .find_by_id(invalid_user_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(welcome_count(&database, invalid_user_id).await, 0);
    }

    let user_id = Uuid::now_v7();
    let email = format!("test-{user_id}@example.com");
    let created = repository
        .create_with_welcome_job(new_user(user_id, format!("  {}  ", email.to_uppercase())))
        .await
        .unwrap();
    assert_eq!(created.email, email);
    assert_eq!(created.role, "user");
    assert_eq!(created.user_id.get_version_num(), 7);
    assert_eq!(created.created_at.timestamp_subsec_nanos() % 1000, 0);
    assert!((Utc::now() - created.created_at).num_seconds().abs() < 30);
    assert_eq!(
        repository
            .find_by_id(user_id)
            .await
            .unwrap()
            .unwrap()
            .user_id,
        user_id
    );
    assert_eq!(
        repository
            .find_credentials_by_email(&email.to_uppercase())
            .await
            .unwrap()
            .unwrap()
            .user
            .user_id,
        user_id
    );
    assert_welcome_job(&database, user_id).await;

    let duplicate_id = Uuid::now_v7();
    assert!(matches!(
        repository
            .create_with_welcome_job(new_user(duplicate_id, email.clone()))
            .await,
        Err(AppError::Conflict(_))
    ));
    assert!(repository.find_by_id(duplicate_id).await.unwrap().is_none());
    assert_eq!(welcome_count(&database, duplicate_id).await, 0);

    let updated_email = format!("updated-{user_id}@example.com");
    let updated = repository
        .update(
            user_id,
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
    assert_eq!(
        repository
            .find_credentials_by_email(&updated_email)
            .await
            .unwrap()
            .unwrap()
            .password_hash,
        "test-only-password-hash"
    );
    assert_eq!(updated.created_at, created.created_at);
    assert!(updated.updated_at >= created.updated_at);
    assert!(
        repository
            .find_credentials_by_email(&email)
            .await
            .unwrap()
            .is_none()
    );
    let unchanged = repository
        .update(
            user_id,
            UserUpdate {
                name: updated.name.clone(),
                email: updated.email.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(unchanged.user_id, user_id);
    assert!(matches!(
        repository
            .update(
                Uuid::now_v7(),
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
    assert_query_budget_and_projections(&database).await;

    assert!(repository.delete(user_id).await.unwrap());
    assert!(!repository.delete(user_id).await.unwrap());
    assert!(repository.find_by_id(user_id).await.unwrap().is_none());
    delete_welcome_job(&database, user_id).await;
    database.close().await;
    assert!(database.ping().await.is_err());
}

async fn assert_transaction_rollback(database: &DatabasePool, repository: Arc<dyn UserRepository>) {
    let user_id = Uuid::now_v7();
    let deduplication_key = format!("welcome:{user_id}");
    // A job collision occurs after the user INSERT, exercising actual rollback.
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query(
                "INSERT INTO jobs (job_id, kind, payload, deduplication_key) VALUES ($1, $2, $3, $4)",
            )
            .bind(Uuid::now_v7())
            .bind("send_welcome_email")
            .bind(Json(json!({"user_id": user_id})))
            .bind(&deduplication_key)
            .execute(pool)
            .await
            .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query(
                "INSERT INTO jobs (job_id, kind, payload, deduplication_key) VALUES (?, ?, ?, ?)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind("send_welcome_email")
            .bind(Json(json!({"user_id": user_id})))
            .bind(&deduplication_key)
            .execute(pool)
            .await
            .unwrap();
        }
    }
    assert!(
        repository
            .create_with_welcome_job(new_user(user_id, format!("rollback-{user_id}@example.com")))
            .await
            .is_err()
    );
    assert!(repository.find_by_id(user_id).await.unwrap().is_none());
    assert_eq!(welcome_count(database, user_id).await, 1);
    delete_welcome_job(database, user_id).await;
}

async fn assert_concurrent_duplicate_registration(
    database: &DatabasePool,
    repository: Arc<dyn UserRepository>,
) {
    let id_a = Uuid::now_v7();
    let id_b = Uuid::now_v7();
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
    assert_welcome_job(database, winner.user_id).await;
    repository.delete(winner.user_id).await.unwrap();
    delete_welcome_job(database, winner.user_id).await;
}

async fn assert_tied_timestamp_pagination(
    database: &DatabasePool,
    repository: Arc<dyn UserRepository>,
) {
    let now = Utc::now();
    let timestamp = (now + Duration::days(365 * 200))
        .with_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap();
    let mut ids = vec![Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
    for user_id in &ids {
        repository
            .create_with_welcome_job(new_user(*user_id, format!("page-{user_id}@example.com")))
            .await
            .unwrap();
        set_created_at(database, *user_id, timestamp).await;
    }
    ids.sort_unstable_by(|left, right| right.cmp(left));
    let start = UserCursor {
        created_at: timestamp,
        user_id: Uuid::from_u128(u128::MAX),
    };
    let first = repository.list(Some(start), 2).await.unwrap();
    assert_eq!(
        first
            .users
            .iter()
            .map(|user| user.user_id)
            .collect::<Vec<_>>(),
        ids[..2]
    );
    let cursor = first.next_cursor.clone().expect("lookahead cursor");
    assert_eq!(cursor.user_id, ids[1]);
    assert_eq!(cursor.created_at, timestamp);

    // A newer insert is visible on a fresh page, not on continuation below the
    // already-issued position. Cursor pagination is not a dataset snapshot.
    let inserted_user_id = Uuid::now_v7();
    repository
        .create_with_welcome_job(new_user(
            inserted_user_id,
            format!("inserted-{inserted_user_id}@example.com"),
        ))
        .await
        .unwrap();
    set_created_at(
        database,
        inserted_user_id,
        timestamp + Duration::microseconds(1),
    )
    .await;

    // Deleting the cursor row must not skip the remaining tied-timestamp row.
    repository.delete(ids[1]).await.unwrap();
    let second = repository.list(Some(cursor), 1).await.unwrap();
    assert_eq!(second.users[0].user_id, ids[2]);
    let observed: HashSet<_> = first
        .users
        .iter()
        .chain(&second.users)
        .map(|user| user.user_id)
        .collect();
    assert_eq!(observed.len(), 3);
    let refreshed = repository.list(None, 1).await.unwrap();
    assert_eq!(refreshed.users[0].user_id, inserted_user_id);
    assert!(repository.list(None, 0).await.is_err());
    assert!(repository.list(None, 101).await.is_err());

    ids.push(inserted_user_id);
    for user_id in ids {
        repository.delete(user_id).await.unwrap();
        delete_welcome_job(database, user_id).await;
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

async fn assert_welcome_job(database: &DatabasePool, user_id: Uuid) {
    let deduplication_key = format!("welcome:{user_id}");
    let (job_id, kind, payload, status): (Uuid, String, Json<Value>, String) = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let row = sqlx::query(
                "SELECT job_id, kind, payload, status FROM jobs WHERE deduplication_key = $1",
            )
            .bind(deduplication_key)
            .fetch_one(pool)
            .await
            .unwrap();
            (
                row.get("job_id"),
                row.get("kind"),
                row.get("payload"),
                row.get("status"),
            )
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let row = sqlx::query(
                "SELECT job_id, kind, payload, status FROM jobs WHERE deduplication_key = ?",
            )
            .bind(deduplication_key)
            .fetch_one(pool)
            .await
            .unwrap();
            let kind: &[u8] = row.get("kind");
            let status: &[u8] = row.get("status");
            let job_id: &[u8] = row.get("job_id");
            (
                Uuid::parse_str(std::str::from_utf8(job_id).unwrap()).unwrap(),
                std::str::from_utf8(kind).unwrap().to_owned(),
                row.get("payload"),
                std::str::from_utf8(status).unwrap().to_owned(),
            )
        }
    };
    assert_eq!(job_id.get_version_num(), 7, "welcome job PK must be UUIDv7");
    assert_eq!(kind, "send_welcome_email");
    assert_eq!(payload.0, json!({ "user_id": user_id }));
    assert_eq!(status, "pending");
}

async fn welcome_count(database: &DatabasePool, user_id: Uuid) -> i64 {
    let deduplication_key = format!("welcome:{user_id}");
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => sqlx::query_scalar(
            "SELECT COUNT(job_id) AS job_count FROM jobs WHERE deduplication_key = $1",
        )
        .bind(deduplication_key)
        .fetch_one(pool)
        .await
        .unwrap(),
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => sqlx::query_scalar(
            "SELECT COUNT(job_id) AS job_count FROM jobs WHERE deduplication_key = ?",
        )
        .bind(deduplication_key)
        .fetch_one(pool)
        .await
        .unwrap(),
    }
}

async fn delete_welcome_job(database: &DatabasePool, user_id: Uuid) {
    let deduplication_key = format!("welcome:{user_id}");
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

async fn set_created_at(database: &DatabasePool, user_id: Uuid, created_at: DateTime<Utc>) {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE users SET created_at = $1 WHERE user_id = $2")
                .bind(created_at)
                .bind(user_id)
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE users SET created_at = ? WHERE user_id = ?")
                .bind(created_at.naive_utc())
                .bind(user_id.to_string())
                .execute(pool)
                .await
                .unwrap();
        }
    }
}

#[derive(Clone, Default)]
struct QueryRecorder(Arc<Mutex<Vec<ObservedQuery>>>);

#[derive(Debug, Default)]
struct ObservedQuery {
    statement: String,
    summary: String,
    rows_returned: u64,
}

impl Visit for ObservedQuery {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "db.statement" => self.statement = value.to_owned(),
            "summary" => self.summary = value.to_owned(),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "rows_returned" {
            self.rows_returned = value;
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl<S: Subscriber> Layer<S> for QueryRecorder {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut query = ObservedQuery::default();
        event.record(&mut query);
        if query.statement.is_empty() {
            query.statement = query.summary.clone();
        }
        query.statement = query
            .statement
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        // Transaction-control messages are separate from the documented data
        // query budget. Statement text contains placeholders, never bind values.
        let transaction_control = query.statement.starts_with("START TRANSACTION")
            || query
                .statement
                .split_whitespace()
                .next()
                .is_some_and(|verb| {
                    matches!(
                        verb,
                        "BEGIN" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" | "RELEASE"
                    )
                });
        if !transaction_control {
            self.0.lock().unwrap().push(query);
        }
    }
}

async fn record_queries<T>(future: impl Future<Output = T>) -> (T, Vec<ObservedQuery>) {
    let recorder = QueryRecorder::default();
    let subscriber = tracing_subscriber::registry().with(recorder.clone());
    let result = future.with_subscriber(subscriber).await;
    let queries = std::mem::take(&mut *recorder.0.lock().unwrap());
    (result, queries)
}

fn assert_public_projection(queries: &[ObservedQuery], expected_count: usize) {
    assert_eq!(
        queries.len(),
        expected_count,
        "data query budget changed: {queries:?}"
    );
    for query in queries {
        assert!(!query.statement.contains("SELECT *"));
        assert!(!query.statement.contains("RETURNING *"));
        assert!(!query.statement.contains(".*"));
        assert!(!query.statement.contains("OFFSET"));
        if query.statement.contains("SELECT ") {
            assert!(
                !query.statement.contains("password_hash"),
                "public query fetched credentials"
            );
        }
        if let Some((_, returned)) = query.statement.split_once(" RETURNING ") {
            assert!(
                !returned.contains("password_hash"),
                "write returned credentials"
            );
        }
    }
}

async fn assert_query_budget_and_projections(database: &DatabasePool) {
    // Production pools disable statement logging. Enable it only on this test
    // pool so the assertion observes executed SQL, with an isolated subscriber.
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let observed_database = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(_) => DatabasePool::Postgres(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .after_connect(|connection, _| {
                    Box::pin(async move {
                        sqlx::query("SET TIME ZONE 'UTC'")
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&url)
                .await
                .unwrap(),
        ),
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(_) => DatabasePool::Mysql(
            sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(1)
                .after_connect(|connection, _| {
                    Box::pin(async move {
                        sqlx::query("SET time_zone = '+00:00'")
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&url)
                .await
                .unwrap(),
        ),
    };
    let repository = user_repository(&observed_database);
    let fixtures: Vec<_> = (0..101)
        .map(|_| {
            let user_id = Uuid::now_v7();
            (user_id, format!("budget-{user_id}@example.com"))
        })
        .collect();
    insert_query_fixtures(database, &fixtures).await;

    for limit in [1, 20, 100] {
        let (first, queries) = record_queries(repository.list(None, limit)).await;
        let first = first.unwrap();
        assert_eq!(first.users.len(), limit as usize);
        assert_public_projection(&queries, 1);
        assert_eq!(queries[0].rows_returned, u64::from(limit + 1));
        let cursor = first.next_cursor.expect("fixture must exercise lookahead");
        let (next, queries) = record_queries(repository.list(Some(cursor), limit)).await;
        assert!(next.unwrap().users.len() <= limit as usize);
        assert_public_projection(&queries, 1);
        assert!(queries[0].rows_returned <= u64::from(limit + 1));
    }

    let (user, queries) = record_queries(repository.find_by_id(fixtures[0].0)).await;
    assert_eq!(user.unwrap().unwrap().user_id, fixtures[0].0);
    assert_public_projection(&queries, 1);
    let (contact, queries) = record_queries(repository.find_contact_by_id(fixtures[0].0)).await;
    assert_eq!(contact.unwrap().unwrap().user_id, fixtures[0].0);
    assert_public_projection(&queries, 1);
    assert!(!queries[0].statement.contains("created_at"));
    assert!(!queries[0].statement.contains("role"));

    let (credentials, queries) =
        record_queries(repository.find_credentials_by_email(&fixtures[0].1)).await;
    assert_eq!(credentials.unwrap().unwrap().user.user_id, fixtures[0].0);
    assert_eq!(queries.len(), 1);
    assert!(queries[0].statement.contains("password_hash"));

    let created_user_id = Uuid::now_v7();
    let (created, queries) = record_queries(repository.create_with_welcome_job(new_user(
        created_user_id,
        format!("projected-{created_user_id}@example.com"),
    )))
    .await;
    assert_eq!(created.unwrap().user_id, created_user_id);
    let (create_budget, update_budget) = match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(_) => (2, 1),
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(_) => (3, 2),
    };
    assert_public_projection(&queries, create_budget);
    let (updated, queries) = record_queries(repository.update(
        created_user_id,
        UserUpdate {
            name: "Budget updated".into(),
            email: format!("projected-updated-{created_user_id}@example.com"),
        },
    ))
    .await;
    assert_eq!(updated.unwrap().user_id, created_user_id);
    assert_public_projection(&queries, update_budget);
    let (deleted, queries) = record_queries(repository.delete(created_user_id)).await;
    assert!(deleted.unwrap());
    assert_public_projection(&queries, 1);
    delete_welcome_job(database, created_user_id).await;

    for invalid_user_id in [Uuid::new_v4(), Uuid::nil()] {
        let (result, queries) = record_queries(repository.create_with_welcome_job(new_user(
            invalid_user_id,
            format!("invalid-{invalid_user_id}@example.com"),
        )))
        .await;
        assert!(matches!(result, Err(AppError::BadRequest(_))));
        assert!(queries.is_empty(), "invalid PK reached the database");
    }
    delete_query_fixtures(database, &fixtures).await;
    observed_database.close().await;
}

async fn insert_query_fixtures(database: &DatabasePool, fixtures: &[(Uuid, String)]) {
    assert_eq!(fixtures.len(), 101);
    assert!(
        fixtures
            .iter()
            .all(|(user_id, _)| user_id.get_version_num() == 7)
    );
    let created_at = DateTime::from_timestamp_micros(
        (Utc::now() + Duration::days(365 * 400)).timestamp_micros(),
    )
    .unwrap();
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
                "INSERT INTO users (user_id, name, email, password_hash, created_at) ",
            );
            query.push_values(fixtures, |mut row, (user_id, email)| {
                row.push_bind(*user_id)
                    .push_bind("Query budget fixture")
                    .push_bind(email)
                    .push_bind("test-only-password-hash")
                    .push_bind(created_at);
            });
            query.build().execute(pool).await.unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut query = sqlx::QueryBuilder::<sqlx::MySql>::new(
                "INSERT INTO users (user_id, name, email, password_hash, created_at) ",
            );
            query.push_values(fixtures, |mut row, (user_id, email)| {
                row.push_bind(user_id.to_string())
                    .push_bind("Query budget fixture")
                    .push_bind(email)
                    .push_bind("test-only-password-hash")
                    .push_bind(created_at.naive_utc());
            });
            query.build().execute(pool).await.unwrap();
        }
    }
}

async fn delete_query_fixtures(database: &DatabasePool, fixtures: &[(Uuid, String)]) {
    assert_eq!(fixtures.len(), 101);
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            let mut query =
                sqlx::QueryBuilder::<sqlx::Postgres>::new("DELETE FROM users WHERE user_id IN (");
            let mut ids = query.separated(", ");
            for (user_id, _) in fixtures {
                ids.push_bind(*user_id);
            }
            ids.push_unseparated(")");
            query.build().execute(pool).await.unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            let mut query =
                sqlx::QueryBuilder::<sqlx::MySql>::new("DELETE FROM users WHERE user_id IN (");
            let mut ids = query.separated(", ");
            for (user_id, _) in fixtures {
                ids.push_bind(user_id.to_string());
            }
            ids.push_unseparated(")");
            query.build().execute(pool).await.unwrap();
        }
    }
}
