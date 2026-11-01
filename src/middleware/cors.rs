use crate::config::CorsConfig;
use axum::http::{HeaderValue, Method, header};
use tower_http::cors::CorsLayer;

pub fn layer(config: &CorsConfig) -> anyhow::Result<CorsLayer> {
    let origins: Vec<HeaderValue> = config
        .allowed_origins
        .iter()
        .map(|value| {
            value
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid CORS origin header"))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT])
        .expose_headers([axum::http::HeaderName::from_static("x-request-id")])
        .max_age(std::time::Duration::from_secs(3600)))
}
