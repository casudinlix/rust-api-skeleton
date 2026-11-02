use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub message: String,
    pub data: T,
}

impl<T> ApiResponse<T> {
    pub fn success(message: impl Into<String>, data: T) -> Self {
        Self {
            success: true,
            message: message.into(),
            data,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ApiCursorResponse<T> {
    pub success: bool,
    pub message: String,
    pub data: Vec<T>,
    pub meta: CursorMeta,
}

impl<T> ApiCursorResponse<T> {
    pub fn success(message: impl Into<String>, data: Vec<T>, next_cursor: Option<String>) -> Self {
        let has_more = next_cursor.is_some();
        Self {
            success: true,
            message: message.into(),
            data,
            meta: CursorMeta {
                next_cursor,
                has_more,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CursorMeta {
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Value>,
}

impl ApiErrorResponse {
    pub fn new(message: impl Into<String>, errors: Option<Value>) -> Self {
        Self {
            success: false,
            message: message.into(),
            errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn success_response_has_only_the_standard_success_envelope() {
        let value = serde_json::to_value(ApiResponse::success("ok", json!({"user_id": "u1"})))
            .unwrap();
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
    fn cursor_response_places_pagination_metadata_outside_data() {
        let value = serde_json::to_value(ApiCursorResponse::success(
            "ok",
            vec![json!({"user_id": "u1"})],
            Some("next-token".into()),
        ))
        .unwrap();
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
    fn cursor_response_reports_last_page_without_inventing_a_cursor() {
        let value = serde_json::to_value(ApiCursorResponse::<Value>::success(
            "ok",
            Vec::new(),
            None,
        ))
        .unwrap();
        assert_eq!(value["meta"]["next_cursor"], Value::Null);
        assert_eq!(value["meta"]["has_more"], false);
    }

    #[test]
    fn error_response_omits_errors_when_no_safe_details_exist() {
        let value = serde_json::to_value(ApiErrorResponse::new("not found", None)).unwrap();
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
