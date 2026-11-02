use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, header},
    middleware,
    routing::{get, post},
};
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveRequestHeadersLayer,
    trace::TraceLayer,
};

use crate::{
    errors::AppError,
    handlers::{auth_handler, health_handler, user_handler},
    middleware::{
        cors, observe_http, rate_limit::auth_rate_limit, request_deadline, security_headers,
    },
    state::AppState,
    swagger,
};

/// Equivalent to Laravel's routes/api.php: HTTP declarations only.
pub fn router(state: AppState) -> anyhow::Result<Router> {
    let auth = Router::new()
        .route("/register", post(auth_handler::register))
        .route("/login", post(auth_handler::login))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_rate_limit,
        ));

    let api = Router::new()
        .nest("/auth", auth)
        .route("/me", get(user_handler::me).put(user_handler::update_me))
        .route("/users", get(user_handler::list).post(user_handler::create))
        .route(
            "/users/{user_id}",
            get(user_handler::show)
                .put(user_handler::update)
                .delete(user_handler::delete),
        );

    let request_id = HeaderName::from_static("x-request-id");
    let layers = ServiceBuilder::new()
        .layer(SetRequestIdLayer::new(request_id.clone(), MakeRequestUuid))
        .layer(PropagateRequestIdLayer::new(request_id))
        .layer(SetSensitiveRequestHeadersLayer::new([header::AUTHORIZATION, header::COOKIE]))
        .layer(middleware::from_fn_with_state(state.clone(), observe_http))
        .layer(TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
            // URI query strings can contain user data or accidental credentials.
            tracing::info_span!("http_request", method = %request.method(), path = %request.uri().path(), request_id = ?request.headers().get("x-request-id"))
        }))
        .layer(middleware::from_fn(security_headers))
        .layer(cors::layer(&state.settings.cors)?)
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn_with_state(state.clone(), request_deadline));

    let mut root = Router::new()
        .route("/health", get(health_handler::health))
        .route("/ready", get(health_handler::ready))
        .nest("/api/v1", api);

    if state.settings.swagger.enabled {
        root = root.merge(swagger::router(&state.settings.swagger));
    }

    Ok(root
        .fallback(|| async { AppError::NotFound("Route not found".into()) })
        .method_not_allowed_fallback(|| async { AppError::MethodNotAllowed })
        .layer(DefaultBodyLimit::max(state.settings.app.body_limit_bytes))
        .layer(layers)
        .with_state(state))
}
