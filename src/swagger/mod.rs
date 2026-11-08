use axum::{Json, Router, extract::State, response::Html, routing::get};
use serde_json::{Value, json};

use crate::{config::swagger::SwaggerConfig, state::AppState};

pub fn router(config: &SwaggerConfig) -> Router<AppState> {
    if !config.enabled {
        return Router::new();
    }

    Router::new()
        .route(config.openapi_path.as_str(), get(openapi))
        .route(config.ui_path.as_str(), get(ui))
}

async fn openapi() -> Json<Value> {
    Json(openapi_document())
}

async fn ui(State(state): State<AppState>) -> Html<String> {
    Html(swagger_ui_html(&state.settings.swagger))
}

pub fn openapi_document() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Rust API Skeleton",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Reference API contract for the reusable Rust API skeleton."
        },
        "tags": [
            {"name": "health", "description": "Process liveness and readiness"},
            {"name": "auth", "description": "Registration and authentication"},
            {"name": "profile", "description": "Authenticated current-user operations"},
            {"name": "users", "description": "Reference administrative user operations"}
        ],
        "paths": {
            "/health": {
                "get": operation(
                    "health.check",
                    "Process liveness",
                    "health",
                    false,
                    None,
                    responses(&[
                        ("200", "Process is alive", "#/components/schemas/HealthResponse")
                    ])
                )
            },
            "/ready": {
                "get": operation(
                    "health.ready",
                    "Database and migration readiness",
                    "health",
                    false,
                    None,
                    responses(&[
                        ("200", "Service is ready", "#/components/schemas/ReadyResponse"),
                        ("503", "Dependency or migration readiness failed", "#/components/schemas/ErrorResponse")
                    ])
                )
            },
            "/api/v1/auth/register": {
                "post": operation(
                    "auth.register",
                    "Register a user",
                    "auth",
                    false,
                    Some(request_body("#/components/schemas/RegisterRequest")),
                    responses(&[
                        ("201", "Registration successful", "#/components/schemas/TokenEnvelope"),
                        ("400", "Malformed or unknown JSON field", "#/components/schemas/ErrorResponse"),
                        ("409", "Registration conflicts with existing data", "#/components/schemas/ErrorResponse"),
                        ("413", "Request body too large", "#/components/schemas/ErrorResponse"),
                        ("415", "Content-Type is not application/json", "#/components/schemas/ErrorResponse"),
                        ("422", "Validation failed", "#/components/schemas/ValidationErrorResponse"),
                        ("429", "Authentication rate limit exceeded", "#/components/schemas/ErrorResponse")
                    ])
                )
            },
            "/api/v1/auth/login": {
                "post": operation(
                    "auth.login",
                    "Authenticate and issue a bearer token",
                    "auth",
                    false,
                    Some(request_body("#/components/schemas/LoginRequest")),
                    responses(&[
                        ("200", "Login successful", "#/components/schemas/TokenEnvelope"),
                        ("400", "Malformed or unknown JSON field", "#/components/schemas/ErrorResponse"),
                        ("401", "Credentials are invalid", "#/components/schemas/ErrorResponse"),
                        ("413", "Request body too large", "#/components/schemas/ErrorResponse"),
                        ("415", "Content-Type is not application/json", "#/components/schemas/ErrorResponse"),
                        ("422", "Validation failed", "#/components/schemas/ValidationErrorResponse"),
                        ("429", "Authentication rate limit exceeded", "#/components/schemas/ErrorResponse")
                    ])
                )
            },
            "/api/v1/me": {
                "get": operation(
                    "profile.show",
                    "Get the authenticated user",
                    "profile",
                    true,
                    None,
                    responses(&[
                        ("200", "Profile retrieved", "#/components/schemas/UserEnvelope"),
                        ("401", "Authentication required", "#/components/schemas/ErrorResponse")
                    ])
                ),
                "put": operation(
                    "profile.update",
                    "Update the authenticated user",
                    "profile",
                    true,
                    Some(request_body("#/components/schemas/UpdateUserRequest")),
                    responses(&[
                        ("200", "Profile updated", "#/components/schemas/UserEnvelope"),
                        ("400", "Malformed or unknown JSON field", "#/components/schemas/ErrorResponse"),
                        ("401", "Authentication required", "#/components/schemas/ErrorResponse"),
                        ("409", "Update conflicts with existing data", "#/components/schemas/ErrorResponse"),
                        ("413", "Request body too large", "#/components/schemas/ErrorResponse"),
                        ("415", "Content-Type is not application/json", "#/components/schemas/ErrorResponse"),
                        ("422", "Validation failed", "#/components/schemas/ValidationErrorResponse")
                    ])
                )
            },
            "/api/v1/users": {
                "get": {
                    "operationId": "users.list",
                    "summary": "List users with cursor pagination",
                    "tags": ["users"],
                    "security": [{"bearerAuth": []}],
                    "parameters": [
                        {
                            "name": "cursor",
                            "in": "query",
                            "required": false,
                            "description": "Opaque versioned cursor returned by the previous page.",
                            "schema": {"type": ["string", "null"], "maxLength": 512}
                        },
                        {
                            "name": "limit",
                            "in": "query",
                            "required": false,
                            "description": "Items per page. Default 20, maximum 100.",
                            "schema": {"type": ["integer", "null"], "minimum": 1, "maximum": 100, "default": 20}
                        }
                    ],
                    "responses": responses(&[
                        ("200", "Users retrieved", "#/components/schemas/UserCursorEnvelope"),
                        ("400", "Invalid cursor or pagination query", "#/components/schemas/ErrorResponse"),
                        ("401", "Authentication required", "#/components/schemas/ErrorResponse"),
                        ("403", "Access forbidden", "#/components/schemas/ErrorResponse")
                    ])
                },
                "post": operation(
                    "users.create",
                    "Create a user",
                    "users",
                    true,
                    Some(request_body("#/components/schemas/RegisterRequest")),
                    responses(&[
                        ("201", "User created", "#/components/schemas/UserEnvelope"),
                        ("400", "Malformed or unknown JSON field", "#/components/schemas/ErrorResponse"),
                        ("401", "Authentication required", "#/components/schemas/ErrorResponse"),
                        ("403", "Access forbidden", "#/components/schemas/ErrorResponse"),
                        ("409", "User conflicts with existing data", "#/components/schemas/ErrorResponse"),
                        ("413", "Request body too large", "#/components/schemas/ErrorResponse"),
                        ("415", "Content-Type is not application/json", "#/components/schemas/ErrorResponse"),
                        ("422", "Validation failed", "#/components/schemas/ValidationErrorResponse")
                    ])
                )
            },
            "/api/v1/users/{user_id}": {
                "parameters": [{
                    "name": "user_id",
                    "in": "path",
                    "required": true,
                    "description": "Application user UUID.",
                    "schema": {"type": "string", "format": "uuid"}
                }],
                "get": operation(
                    "users.show",
                    "Get one user",
                    "users",
                    true,
                    None,
                    responses(&[
                        ("200", "User retrieved", "#/components/schemas/UserEnvelope"),
                        ("400", "Invalid UUID", "#/components/schemas/ErrorResponse"),
                        ("401", "Authentication required", "#/components/schemas/ErrorResponse"),
                        ("403", "Access forbidden", "#/components/schemas/ErrorResponse"),
                        ("404", "User not found", "#/components/schemas/ErrorResponse")
                    ])
                ),
                "put": operation(
                    "users.update",
                    "Update one user",
                    "users",
                    true,
                    Some(request_body("#/components/schemas/UpdateUserRequest")),
                    responses(&[
                        ("200", "User updated", "#/components/schemas/UserEnvelope"),
                        ("400", "Malformed input or invalid UUID", "#/components/schemas/ErrorResponse"),
                        ("401", "Authentication required", "#/components/schemas/ErrorResponse"),
                        ("403", "Access forbidden", "#/components/schemas/ErrorResponse"),
                        ("404", "User not found", "#/components/schemas/ErrorResponse"),
                        ("409", "Update conflicts with existing data", "#/components/schemas/ErrorResponse"),
                        ("413", "Request body too large", "#/components/schemas/ErrorResponse"),
                        ("415", "Content-Type is not application/json", "#/components/schemas/ErrorResponse"),
                        ("422", "Validation failed", "#/components/schemas/ValidationErrorResponse")
                    ])
                ),
                "delete": operation(
                    "users.delete",
                    "Delete one user",
                    "users",
                    true,
                    None,
                    responses(&[
                        ("200", "User deleted", "#/components/schemas/DeleteEnvelope"),
                        ("400", "Invalid UUID", "#/components/schemas/ErrorResponse"),
                        ("401", "Authentication required", "#/components/schemas/ErrorResponse"),
                        ("403", "Access forbidden", "#/components/schemas/ErrorResponse"),
                        ("404", "User not found", "#/components/schemas/ErrorResponse"),
                        ("409", "Administrator cannot delete their own account", "#/components/schemas/ErrorResponse")
                    ])
                )
            }
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "description": "Access token returned by /api/v1/auth/login or /register."
                }
            },
            "schemas": schemas()
        }
    })
}

fn operation(
    operation_id: &str,
    summary: &str,
    tag: &str,
    secured: bool,
    body: Option<Value>,
    responses: Value,
) -> Value {
    let mut operation = json!({
        "operationId": operation_id,
        "summary": summary,
        "tags": [tag],
        "responses": responses
    });
    if secured {
        operation["security"] = json!([{"bearerAuth": []}]);
    }
    if let Some(body) = body {
        operation["requestBody"] = body;
    }
    operation
}

fn request_body(schema: &str) -> Value {
    json!({
        "required": true,
        "content": {
            "application/json": {
                "schema": {"$ref": schema}
            }
        }
    })
}

fn responses(rows: &[(&str, &str, &str)]) -> Value {
    let mut object = serde_json::Map::new();
    for (status, description, schema) in rows {
        object.insert(
            (*status).to_owned(),
            json!({
                "description": description,
                "content": {
                    "application/json": {
                        "schema": {"$ref": schema}
                    }
                }
            }),
        );
    }
    Value::Object(object)
}

fn schemas() -> Value {
    json!({
        "RegisterRequest": {
            "type": "object",
            "additionalProperties": false,
            "required": ["name", "email", "password"],
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 100, "example": "Ada Lovelace"},
                "email": {"type": "string", "format": "email", "maxLength": 254, "example": "ada@example.com"},
                "password": {
                    "type": "string",
                    "format": "password",
                    "writeOnly": true,
                    "description": "12 to 128 bytes; the server does not trim or normalize passwords.",
                    "example": "example-password-only"
                }
            }
        },
        "LoginRequest": {
            "type": "object",
            "additionalProperties": false,
            "required": ["email", "password"],
            "properties": {
                "email": {"type": "string", "format": "email", "maxLength": 254, "example": "ada@example.com"},
                "password": {
                    "type": "string",
                    "format": "password",
                    "writeOnly": true,
                    "description": "1 to 128 bytes; the server does not normalize passwords.",
                    "example": "example-password-only"
                }
            }
        },
        "UpdateUserRequest": {
            "type": "object",
            "additionalProperties": false,
            "required": ["name", "email"],
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 100, "example": "Ada Lovelace"},
                "email": {"type": "string", "format": "email", "maxLength": 254, "example": "ada@example.com"}
            }
        },
        "UserResponse": {
            "type": "object",
            "additionalProperties": false,
            "required": ["user_id", "name", "email", "role", "created_at", "updated_at"],
            "properties": {
                "user_id": {"type": "string", "format": "uuid", "example": "019a3b7c-8d2f-7abc-8def-0123456789ab"},
                "name": {"type": "string", "example": "Ada Lovelace"},
                "email": {"type": "string", "format": "email", "example": "ada@example.com"},
                "role": {"type": "string", "example": "user"},
                "created_at": {"type": "string", "format": "date-time", "example": "2026-11-02T04:00:00Z"},
                "updated_at": {"type": "string", "format": "date-time", "example": "2026-11-02T04:00:00Z"}
            }
        },
        "TokenResponse": {
            "type": "object",
            "additionalProperties": false,
            "required": ["access_token", "token_type", "expires_in", "user"],
            "properties": {
                "access_token": {"type": "string", "readOnly": true, "example": "example.jwt.token"},
                "token_type": {"type": "string", "const": "Bearer"},
                "expires_in": {"type": "integer", "minimum": 60, "example": 900},
                "user": {"$ref": "#/components/schemas/UserResponse"}
            }
        },
        "ErrorResponse": {
            "type": "object",
            "additionalProperties": false,
            "required": ["success", "code", "message"],
            "properties": {
                "success": {"type": "boolean", "const": false},
                "code": {"type": "string", "example": "NOT_FOUND"},
                "message": {"type": "string", "example": "Route not found"}
            }
        },
        "ValidationErrorResponse": {
            "type": "object",
            "additionalProperties": false,
            "required": ["success", "code", "message", "errors"],
            "properties": {
                "success": {"type": "boolean", "const": false},
                "code": {"type": "string", "const": "VALIDATION_FAILED"},
                "message": {"type": "string", "const": "Validation failed"},
                "errors": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["code", "message"],
                            "properties": {
                                "code": {"type": "string", "example": "email"},
                                "message": {"type": "string", "example": "must be a valid email address"}
                            }
                        }
                    }
                }
            }
        },
        "UserEnvelope": success_envelope(json!({"$ref": "#/components/schemas/UserResponse"})),
        "TokenEnvelope": success_envelope(json!({"$ref": "#/components/schemas/TokenResponse"})),
        "DeleteEnvelope": success_envelope(json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["deleted"],
            "properties": {"deleted": {"type": "boolean", "const": true}}
        })),
        "HealthResponse": success_envelope(json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["status"],
            "properties": {"status": {"type": "string", "const": "ok"}}
        })),
        "ReadyResponse": success_envelope(json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["status", "database"],
            "properties": {
                "status": {"type": "string", "const": "ready"},
                "database": {"type": "string", "const": "ok"}
            }
        })),
        "UserCursorEnvelope": {
            "type": "object",
            "additionalProperties": false,
            "required": ["success", "message", "data", "meta"],
            "properties": {
                "success": {"type": "boolean", "const": true},
                "message": {"type": "string", "example": "Users retrieved"},
                "data": {
                    "type": "array",
                    "items": {"$ref": "#/components/schemas/UserResponse"}
                },
                "meta": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["next_cursor", "has_more"],
                    "properties": {
                        "next_cursor": {
                            "type": ["string", "null"],
                            "example": "eyJ2IjoxLCJyZXNvdXJjZSI6InVzZXJzIn0"
                        },
                        "has_more": {"type": "boolean", "example": true}
                    }
                }
            }
        }
    })
}

fn success_envelope(data: Value) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["success", "message", "data"],
        "properties": {
            "success": {"type": "boolean", "const": true},
            "message": {"type": "string"},
            "data": data
        }
    })
}

fn swagger_ui_html(config: &SwaggerConfig) -> String {
    let assets = config.asset_base_url.trim_end_matches('/');
    let openapi_path =
        serde_json::to_string(&config.openapi_path).expect("validated Swagger path serializes");
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Rust API Skeleton - Swagger UI</title>
  <link rel="stylesheet" href="{assets}/swagger-ui.css">
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="{assets}/swagger-ui-bundle.js"></script>
  <script src="{assets}/swagger-ui-standalone-preset.js"></script>
  <script>
    window.ui = SwaggerUIBundle({{
      url: {openapi_path},
      dom_id: '#swagger-ui',
      deepLinking: true,
      persistAuthorization: false,
      displayRequestDuration: true,
      presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
      layout: 'StandaloneLayout'
    }});
  </script>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_contains_current_reference_routes_and_bearer_security() {
        let document = openapi_document();
        for path in [
            "/health",
            "/ready",
            "/api/v1/auth/register",
            "/api/v1/auth/login",
            "/api/v1/me",
            "/api/v1/users",
            "/api/v1/users/{user_id}",
        ] {
            assert!(document["paths"].get(path).is_some(), "missing {path}");
        }
        assert_eq!(
            document["components"]["securitySchemes"]["bearerAuth"]["scheme"],
            "bearer"
        );
    }

    #[test]
    fn openapi_contract_uses_named_pk_cursor_envelope_and_safe_examples() {
        let document = openapi_document();
        let encoded = document.to_string();
        assert!(encoded.contains("user_id"));
        assert!(encoded.contains("next_cursor"));
        assert!(encoded.contains("has_more"));
        for forbidden in [
            "password_hash",
            "JWT_SECRET",
            "SMTP_PASSWORD",
            "DATABASE_URL",
            "private-password-hash",
        ] {
            assert!(!encoded.contains(forbidden), "OpenAPI leaked {forbidden}");
        }
    }

    #[test]
    fn swagger_ui_does_not_persist_bearer_authorization() {
        let html = swagger_ui_html(&SwaggerConfig {
            enabled: true,
            ui_path: "/docs".into(),
            openapi_path: "/api-docs/openapi.json".into(),
            asset_base_url: "https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.32.6".into(),
        });
        assert!(html.contains("persistAuthorization: false"));
        assert!(html.contains("/api-docs/openapi.json"));
        assert!(html.contains("swagger-ui-bundle.js"));
    }
}
