use serde::Deserialize;

use crate::error::AppError;

#[derive(Debug, Deserialize)]
pub struct ApiErrorResponse {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

pub fn map_status_to_error(status: u16, body: &str, retry_after_seconds: Option<u64>) -> AppError {
    let detail = if let Ok(api_err) = serde_json::from_str::<ApiErrorResponse>(body) {
        api_err
            .error_description
            .or(api_err.message)
            .or(api_err.error)
            .unwrap_or_else(|| body.to_string())
    } else {
        body.to_string()
    };

    match status {
        401 | 403 => AppError::Auth(detail),
        400 | 413 | 415 | 422 => AppError::InvalidInput(detail),
        404 => AppError::NotFound(detail),
        409 | 412 | 428 => AppError::Conflict(detail),
        429 => AppError::RateLimited {
            retry_after_seconds,
        },
        500..=599 => AppError::ServerError(detail),
        _ => AppError::ServerError(format!("HTTP {status}: {detail}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_current_validation_and_concurrency_statuses() {
        assert_eq!(
            map_status_to_error(400, r#"{"message":"bad field"}"#, None).exit_code(),
            2
        );
        assert_eq!(map_status_to_error(413, "too large", None).exit_code(), 2);
        assert_eq!(map_status_to_error(415, "json only", None).exit_code(), 2);
        assert_eq!(map_status_to_error(412, "stale", None).exit_code(), 3);
        assert_eq!(map_status_to_error(428, "required", None).exit_code(), 3);
    }
}
