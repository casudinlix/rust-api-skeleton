use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub message: String,
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Value>,
}

impl<T> ApiResponse<T> {
    pub fn success(message: impl Into<String>, data: T) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: Some(data),
            errors: None,
        }
    }
}
