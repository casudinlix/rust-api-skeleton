//! Full HTTP/service/repository tests. Use only a disposable test database.

mod support;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rust_api_skeleton::{
    config::Settings, database::DatabasePool, routes::api::router, state::AppState,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for a disposable PostgreSQL database"]
async fn postgres_api() {
    exercise("postgres").await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for a disposable MySQL database"]
async fn mysql_api() {
    exercise("mysql").await;
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
    status: StatusCode,
) -> Value {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = if let Some(body) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let actual = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(actual, status, "{method} {path}: {value}");
    value
}

async fn exercise(driver: &str) {
    let source = config::Config::builder()
        .set_override("db_driver", driver)
        .unwrap()
        .set_override(
            "database_url",
            support::database_url(driver.parse().expect("known database driver"))
                .expect("load test database URL"),
        )
        .unwrap()
        .set_override(
            "jwt_secret",
            "local-integration-secret-over-thirty-two-bytes",
        )
        .unwrap()
        .set_override("auth_requests_per_minute", 100)
        .unwrap()
        .build()
        .unwrap();
    let mut settings = Settings::from_config(source).unwrap();
    let admin = DatabasePool::connect(&settings.database).await.unwrap();
    let database_name = format!("api_test_{}", Uuid::new_v4().simple());
    match &admin {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query(&format!("CREATE DATABASE {database_name}"))
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query(&format!("CREATE DATABASE {database_name}"))
                .execute(pool)
                .await
                .unwrap();
        }
    }
    let mut url = url::Url::parse(&settings.database.url).unwrap();
    url.set_path(&database_name);
    settings.database.url = url.to_string();
    let database = DatabasePool::connect(&settings.database).await.unwrap();
    let app = router(AppState::new(settings, database.clone()).unwrap()).unwrap();
    call(&app, "GET", "/health", None, None, StatusCode::OK).await;
    call(
        &app,
        "GET",
        "/ready",
        None,
        None,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    database.migrate().await.unwrap();
    call(&app, "GET", "/ready", None, None, StatusCode::OK).await;
    match &database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE _sqlx_migrations SET success=FALSE WHERE version=20260101000000")
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE _sqlx_migrations SET success=FALSE WHERE version=20260101000000")
                .execute(pool)
                .await
                .unwrap();
        }
    }
    call(
        &app,
        "GET",
        "/ready",
        None,
        None,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    match &database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE _sqlx_migrations SET success=TRUE WHERE version=20260101000000")
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE _sqlx_migrations SET success=TRUE WHERE version=20260101000000")
                .execute(pool)
                .await
                .unwrap();
        }
    }
    call(&app, "GET", "/ready", None, None, StatusCode::OK).await;
    let email = format!("api-{}@example.com", Uuid::new_v4());
    let registration = json!({"name":"  Ada  ", "email":format!(" {} ",email.to_uppercase()), "password":"test-password-long-enough"});
    let response = call(
        &app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(registration.clone()),
        StatusCode::CREATED,
    )
    .await;
    let token = response["data"]["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let user_id = Uuid::parse_str(response["data"]["user"]["user_id"].as_str().unwrap()).unwrap();
    assert_eq!(user_id.get_version_num(), 7);
    assert!(response["data"]["user"].get("id").is_none());
    assert_eq!(response["data"]["user"]["email"], email);
    assert_eq!(response["data"]["user"]["name"], "Ada");
    assert_eq!(response["data"]["user"]["role"], "user");
    assert!(!response.to_string().contains("password_hash"));
    call(
        &app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(registration),
        StatusCode::CONFLICT,
    )
    .await;
    call(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"email":email,"password":"wrong-password"})),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    call(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"email":email,"password":"test-password-long-enough"})),
        StatusCode::OK,
    )
    .await;
    call(
        &app,
        "GET",
        "/api/v1/me",
        Some("invalid-token"),
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    call(
        &app,
        "GET",
        "/api/v1/me",
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    call(
        &app,
        "GET",
        "/api/v1/users",
        Some(&token),
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    call(
        &app,
        "GET",
        &format!("/api/v1/users/{user_id}"),
        Some(&token),
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    call(
        &app,
        "PUT",
        "/api/v1/me",
        Some(&token),
        Some(json!({"name":"Updated Ada","email":email})),
        StatusCode::OK,
    )
    .await;

    // Administrative provisioning is an explicit DB operation, never a public DTO.
    match &database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE users SET role='admin' WHERE user_id=$1")
                .bind(user_id)
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE users SET role='admin' WHERE user_id=?")
                .bind(user_id.to_string())
                .execute(pool)
                .await
                .unwrap();
        }
    }
    // Existing JWT observes the new database role immediately.
    let response = call(
        &app,
        "GET",
        "/api/v1/users?limit=1",
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(response["data"].as_array().unwrap().len(), 1);
    call(
        &app,
        "GET",
        "/api/v1/users?limit=0",
        Some(&token),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
    call(
        &app,
        "GET",
        "/api/v1/users?cursor=invalid",
        Some(&token),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
    call(
        &app,
        "GET",
        "/api/v1/users/not-a-uuid",
        Some(&token),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let created = call(&app,"POST","/api/v1/users",Some(&token),Some(json!({"name":"Managed User","email":format!("managed-{user_id}@example.com"),"password":"another-long-test-password"})),StatusCode::CREATED).await;
    assert!(created["data"].get("access_token").is_none());
    let managed_id = Uuid::parse_str(created["data"]["user_id"].as_str().unwrap()).unwrap();
    assert_eq!(managed_id.get_version_num(), 7);
    assert!(created["data"].get("id").is_none());
    let page = call(
        &app,
        "GET",
        "/api/v1/users?limit=1",
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(page["meta"]["has_more"], true);
    let cursor = page["meta"]["next_cursor"].as_str().unwrap();
    let envelope: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor).unwrap()).unwrap();
    assert_eq!(envelope["version"], 1);
    assert_eq!(envelope["resource"], "users");
    assert_eq!(envelope["scope"], "admin:all");
    assert_eq!(envelope["sort"], "created_at_desc_user_id_desc");
    let next_page = call(
        &app,
        "GET",
        &format!("/api/v1/users?limit=1&cursor={cursor}"),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(next_page["data"].as_array().unwrap().len(), 1);
    assert_ne!(
        page["data"][0]["user_id"],
        next_page["data"][0]["user_id"]
    );
    assert!(next_page["meta"]["next_cursor"].is_null());
    assert_eq!(next_page["meta"]["has_more"], false);
    for (field, value) in [
        ("version", json!(2)),
        ("resource", json!("jobs")),
        ("scope", json!("tenant:other")),
        ("sort", json!("user_id_asc")),
    ] {
        let mut invalid = envelope.clone();
        invalid[field] = value;
        let cursor = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invalid).unwrap());
        call(
            &app,
            "GET",
            &format!("/api/v1/users?cursor={cursor}"),
            Some(&token),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let legacy_cursor = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(
            &json!({"id": user_id, "created_at": response["data"][0]["created_at"]}),
        )
        .unwrap(),
    );
    call(
        &app,
        "GET",
        &format!("/api/v1/users?cursor={legacy_cursor}"),
        Some(&token),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let path = format!("/api/v1/users/{managed_id}");
    call(&app, "GET", &path, Some(&token), None, StatusCode::OK).await;
    call(&app,"PUT",&path,Some(&token),Some(json!({"name":"Managed Updated","email":format!("managed-updated-{user_id}@example.com")})),StatusCode::OK).await;
    call(
        &app,
        "DELETE",
        &format!("/api/v1/users/{user_id}"),
        Some(&token),
        None,
        StatusCode::CONFLICT,
    )
    .await;
    call(&app, "DELETE", &path, Some(&token), None, StatusCode::OK).await;
    call(
        &app,
        "GET",
        &path,
        Some(&token),
        None,
        StatusCode::NOT_FOUND,
    )
    .await;

    // Revocation is observed by the same JWT; the cursor's admin:all scope is
    // never allowed to grant an administrative role.
    match &database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query("UPDATE users SET role='user' WHERE user_id=$1")
                .bind(user_id)
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query("UPDATE users SET role='user' WHERE user_id=?")
                .bind(user_id.to_string())
                .execute(pool)
                .await
                .unwrap();
        }
    }
    call(
        &app,
        "GET",
        &format!("/api/v1/users?cursor={cursor}"),
        Some(&token),
        None,
        StatusCode::FORBIDDEN,
    )
    .await;

    for target_user_id in [user_id, managed_id] {
        match &database {
            #[cfg(feature = "postgres")]
            DatabasePool::Postgres(pool) => {
                sqlx::query("DELETE FROM users WHERE user_id=$1")
                    .bind(target_user_id)
                    .execute(pool)
                    .await
                    .unwrap();
                sqlx::query("DELETE FROM jobs WHERE deduplication_key=$1")
                    .bind(format!("welcome:{target_user_id}"))
                    .execute(pool)
                    .await
                    .unwrap();
            }
            #[cfg(feature = "mysql")]
            DatabasePool::Mysql(pool) => {
                sqlx::query("DELETE FROM users WHERE user_id=?")
                    .bind(target_user_id.to_string())
                    .execute(pool)
                    .await
                    .unwrap();
                sqlx::query("DELETE FROM jobs WHERE deduplication_key=?")
                    .bind(format!("welcome:{target_user_id}"))
                    .execute(pool)
                    .await
                    .unwrap();
            }
        }
    }
    call(
        &app,
        "GET",
        "/api/v1/me",
        Some(&token),
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    drop(app);
    database.close().await;
    match &admin {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => {
            sqlx::query(&format!("DROP DATABASE {database_name}"))
                .execute(pool)
                .await
                .unwrap();
        }
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => {
            sqlx::query(&format!("DROP DATABASE {database_name}"))
                .execute(pool)
                .await
                .unwrap();
        }
    }
    admin.close().await;
}
