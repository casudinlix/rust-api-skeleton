use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use rust_api_skeleton::{
    config::Settings, database::DatabasePool, routes::api::router, state::AppState,
};
use serde_json::{Value, json};
use tower::ServiceExt;

fn app(limit: u32) -> Router {
    let (driver, url) = if cfg!(feature = "postgres") {
        ("postgres", "postgres://test:test@127.0.0.1:9/test")
    } else {
        ("mysql", "mysql://test:test@127.0.0.1:9/test")
    };
    let settings = Settings::from_config(
        config::Config::builder()
            .set_override("db_driver", driver)
            .unwrap()
            .set_override("database_url", url)
            .unwrap()
            .set_override("jwt_secret", "test-only-key-longer-than-thirty-two-bytes")
            .unwrap()
            .set_override("auth_requests_per_minute", limit)
            .unwrap()
            .set_override("db_acquire_timeout_secs", 1)
            .unwrap()
            .build()
            .unwrap(),
    )
    .unwrap();
    #[cfg(feature = "postgres")]
    let database = DatabasePool::Postgres(
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy(url)
            .unwrap(),
    );
    #[cfg(all(feature = "mysql", not(feature = "postgres")))]
    let database = DatabasePool::Mysql(
        sqlx::mysql::MySqlPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy(url)
            .unwrap(),
    );
    router(AppState::new(settings, database).unwrap()).unwrap()
}

async fn request(
    app: Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut request = Request::builder().method(method).uri(path);
    let body = if let Some(body) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = app.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 100_000).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap();
    (status, headers, json)
}

#[tokio::test]
async fn health_is_json_and_independent_of_database() {
    let (status, headers, body) = request(app(20), "GET", "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "ok");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert!(headers.contains_key("x-request-id"));
}

#[tokio::test]
async fn not_found_and_method_not_allowed_use_envelope() {
    for (method, path, expected) in [
        ("GET", "/unknown", StatusCode::NOT_FOUND),
        ("POST", "/health", StatusCode::METHOD_NOT_ALLOWED),
    ] {
        let (status, _, body) = request(app(20), method, path, None).await;
        assert_eq!(status, expected);
        assert_eq!(body["success"], false);
        assert!(body["message"].is_string());
    }
}

#[tokio::test]
async fn protected_routes_require_authentication_before_database_access() {
    for path in ["/api/v1/me", "/api/v1/users", "/api/v1/users/not-a-uuid"] {
        let (status, headers, body) = request(app(20), "GET", path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer");
        assert_eq!(body["success"], false);
    }
}

#[tokio::test]
async fn validation_returns_422_without_password_values() {
    let (status, _, body) = request(
        app(20),
        "POST",
        "/api/v1/auth/register",
        Some(json!({"name":" ","email":"invalid-private-email","password":"secret"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["errors"]["password"].is_array());
    assert!(!body.to_string().contains("secret"));
    assert!(!body.to_string().contains("invalid-private-email"));
}

#[tokio::test]
async fn mass_assignment_is_rejected() {
    for (field, value) in [
        ("role", json!("admin")),
        ("id", json!(uuid::Uuid::now_v7())),
        ("user_id", json!(uuid::Uuid::now_v7())),
    ] {
        let mut body =
            json!({"name":"User","email":"user@example.com","password":"long-test-password"});
        body[field] = value;
        let (status, _, _) = request(app(20), "POST", "/api/v1/auth/register", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn body_rejections_are_json_and_preserve_status() {
    let response = app(20)
        .oneshot(
            Request::post("/api/v1/auth/login")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let response = app(20)
        .oneshot(
            Request::post("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app(20)
        .oneshot(
            Request::post("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("x".repeat(70_000)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
}

#[tokio::test]
async fn auth_rate_limit_also_counts_invalid_requests() {
    let app = app(1);
    let first = request(app.clone(), "POST", "/api/v1/auth/login", Some(json!({}))).await;
    assert_eq!(first.0, StatusCode::BAD_REQUEST);
    let (status, headers, _) = request(app, "POST", "/api/v1/auth/login", Some(json!({}))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(headers[header::RETRY_AFTER], "60");
}

#[tokio::test]
async fn cors_only_allows_configured_origin() {
    for (origin, allowed) in [
        ("http://localhost:5173", true),
        ("https://untrusted.example", false),
    ] {
        let response = app(20)
            .oneshot(
                Request::get("/health")
                    .header(header::ORIGIN, origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            allowed
        );
    }
}

#[tokio::test]
async fn readiness_reports_database_outage_as_503() {
    let (status, _, body) = request(app(20), "GET", "/ready", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["message"], "Service temporarily unavailable");
}
