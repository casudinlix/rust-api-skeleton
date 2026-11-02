use axum::{Json, http::StatusCode};
use serde::Serialize;
use serde_json::Value;

pub type ApiJson<T> = Json<ApiResponse<T>>;
pub type ApiStatusJson<T> = (StatusCode, ApiJson<T>);
pub type ApiCursorJson<T> = Json<ApiCursorResponse<T>>;
pub type ApiErrorJson = Json<ApiErrorResponse>;

pub fn success<T>(message: impl Into<String>, data: T) -> ApiJson<T> {
    Json(ApiResponse {
        success: true,
        message: message.into(),
        data,
    })
}

pub fn success_with_status<T>(
    status: StatusCode,
    message: impl Into<String>,
    data: T,
) -> ApiStatusJson<T> {
    (status, success(message, data))
}

pub fn created<T>(message: impl Into<String>, data: T) -> ApiStatusJson<T> {
    success_with_status(StatusCode::CREATED, message, data)
}

pub fn cursor<T>(
    message: impl Into<String>,
    data: Vec<T>,
    next_cursor: Option<String>,
) -> ApiCursorJson<T> {
    let has_more = next_cursor.is_some();
    Json(ApiCursorResponse {
        success: true,
        message: message.into(),
        data,
        meta: CursorMeta {
            next_cursor,
            has_more,
        },
    })
}

pub fn error(message: impl Into<String>, errors: Option<Value>) -> ApiErrorJson {
    Json(ApiErrorResponse {
        success: false,
        message: message.into(),
        errors,
    })
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    success: bool,
    message: String,
    data: T,
}

#[derive(Debug, Serialize)]
pub struct ApiCursorResponse<T> {
    success: bool,
    message: String,
    data: Vec<T>,
    meta: CursorMeta,
}

#[derive(Debug, Serialize)]
struct CursorMeta {
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorResponse {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<Value>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn success_helper_builds_the_standard_success_envelope() {
        let Json(payload) = success("ok", json!({"user_id": "u1"}));
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(
            value,
            json!({
                "success": true,
                "message": "ok",
                "data": {"user_id": "u1"}
            })
        );
    }

    #[test]
    fn created_helper_keeps_the_same_envelope_with_created_status() {
        let (status, Json(payload)) = created("created", json!({"user_id": "u1"}));
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "success": true,
                "message": "created",
                "data": {"user_id": "u1"}
            })
        );
    }

    #[test]
    fn cursor_helper_places_pagination_metadata_outside_data() {
        let Json(payload) = cursor(
            "ok",
            vec![json!({"user_id": "u1"})],
            Some("next-token".into()),
        );
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(
            value,
            json!({
                "success": true,
                "message": "ok",
                "data": [{"user_id": "u1"}],
                "meta": {
                    "next_cursor": "next-token",
                    "has_more": true
                }
            })
        );
    }

    #[test]
    fn cursor_helper_reports_last_page_without_inventing_a_cursor() {
        let Json(payload) = cursor::<Value>("ok", Vec::new(), None);
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(value["meta"]["next_cursor"], Value::Null);
        assert_eq!(value["meta"]["has_more"], false);
    }

    #[test]
    fn error_helper_omits_errors_when_no_safe_details_exist() {
        let Json(payload) = error("not found", None);
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(
            value,
            json!({
                "success": false,
                "message": "not found"
            })
        );
        assert!(value.get("data").is_none());
    }
}
