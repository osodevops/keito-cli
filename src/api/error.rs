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

pub fn map_status_to_error(status: u16, body: &str) -> AppError {
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
        404 => AppError::NotFound(detail),
        409 => AppError::Conflict(detail),
        422 => AppError::InvalidInput(detail),
        429 => AppError::RateLimited,
        500..=599 => AppError::ServerError(detail),
        _ => AppError::ServerError(format!("HTTP {status}: {detail}")),
    }
}
