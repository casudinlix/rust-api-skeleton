use std::io::{self, Write};

use serde_json::Value;

use crate::errors::{AppError, AppResult};

const MAX_JOB_BYTES: usize = 65_536;

/// Count the serialized payload without allocating another unbounded buffer.
pub(crate) fn validate_job(kind: &str, payload: &Value, key: Option<&str>) -> AppResult<()> {
    if kind.is_empty()
        || kind.len() > 64
        || !kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(AppError::BadRequest("invalid job kind".into()));
    }
    if key.is_some_and(|key| {
        key.is_empty()
            || key.len() > 191
            || !key.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.')
            })
    }) {
        return Err(AppError::BadRequest("invalid job deduplication key".into()));
    }
    serde_json::to_writer(&mut PayloadSize(0), payload)
        .map_err(|_| AppError::BadRequest("job payload exceeds 65536 serialized bytes".into()))
}

struct PayloadSize(usize);

impl Write for PayloadSize {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        let next = self.0.saturating_add(value.len());
        if next > MAX_JOB_BYTES {
            return Err(io::Error::other("job payload limit"));
        }
        self.0 = next;
        Ok(value.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Never persist raw provider or database diagnostics, including caller errors.
pub(crate) fn safe_failure_reason(reason: &str) -> &'static str {
    match reason {
        "invalid_job_payload" => "invalid_job_payload",
        "unsupported_job_kind" => "unsupported_job_kind",
        "user_lookup_failed" => "user_lookup_failed",
        "invalid_recipient" => "invalid_recipient",
        "message_build_failed" => "message_build_failed",
        "smtp_delivery_failed" => "smtp_delivery_failed",
        "maintenance_failed" => "maintenance_failed",
        "job_timed_out" => "job_timed_out",
        "attempt_limit_reached" => "attempt_limit_reached",
        "handler_failed" => "handler_failed",
        _ => "job_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn payload_limit_counts_serialized_escapes_and_rejects_large_jobs() {
        validate_job("test_job", &json!({"ok":true}), Some("tenant:job_1")).unwrap();
        assert!(validate_job("test_job", &json!("x".repeat(MAX_JOB_BYTES)), None).is_err());
        assert!(validate_job("test_job", &json!("\n".repeat(MAX_JOB_BYTES / 2)), None).is_err());
        assert!(validate_job("invalid kind", &json!({}), None).is_err());
        assert!(validate_job("test_job", &json!({}), Some("token\nsecret")).is_err());
    }

    #[test]
    fn arbitrary_failure_text_is_never_retained() {
        assert_eq!(
            safe_failure_reason("SMTP user@example.org password=secret"),
            "job_failed"
        );
        assert_eq!(safe_failure_reason("secretpassword"), "job_failed");
        assert_eq!(
            safe_failure_reason("smtp_delivery_failed"),
            "smtp_delivery_failed"
        );
    }
}
