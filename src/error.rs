use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Rate limited — please retry after a moment")]
    RateLimited,

    #[error("Server error: {0}")]
    ServerError(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl AppError {
    pub fn exit_code(&self) -> i32 {
        match self {
            AppError::Auth(_) => 1,
            AppError::InvalidInput(_) => 2,
            AppError::Conflict(_) => 3,
            AppError::NotFound(_) => 4,
            AppError::RateLimited => 5,
            AppError::ServerError(_) => 6,
            AppError::Network(_) => 7,
            AppError::Config(_) => 8,
        }
    }

    pub fn suggestion(&self) -> Option<&str> {
        match self {
            AppError::Conflict(msg) if msg.contains("already running") => Some("keito time stop"),
            AppError::NotFound(msg) if msg.contains("No running timer") => {
                Some("keito time start --project <ID> --task <ID>")
            }
            AppError::NotFound(msg) if msg.contains("Project") => {
                Some("keito projects list --json")
            }
            AppError::NotFound(msg) if msg.contains("Task") => Some("keito projects tasks --json"),
            AppError::Auth(_) => Some("Set KEITO_API_KEY env var or run 'keito auth login'"),
            AppError::Config(_) => Some("Run 'keito auth login' to configure"),
            AppError::RateLimited => Some("Retry after a moment"),
            _ => None,
        }
    }

    pub fn details(&self) -> Option<serde_json::Value> {
        match self {
            AppError::NotFound(msg) => {
                if let Some(available_str) = msg
                    .strip_suffix(|_: char| false)
                    .or(Some(msg))
                    .and_then(|m| {
                        m.find("Available: ")
                            .map(|idx| &m[idx + "Available: ".len()..])
                    })
                {
                    let names: Vec<&str> = available_str.split(", ").collect();
                    if !names.is_empty() {
                        return Some(json!({"available": names}));
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn to_json(&self) -> String {
        let mut map = serde_json::Map::new();
        map.insert("error".into(), json!(true));
        map.insert("code".into(), json!(self.exit_code()));
        map.insert("message".into(), json!(self.to_string()));
        if let Some(s) = self.suggestion() {
            map.insert("suggestion".into(), json!(s));
        }
        if let Some(d) = self.details() {
            map.insert("details".into(), d);
        }
        let val = serde_json::Value::Object(map);
        serde_json::to_string_pretty(&val)
            .unwrap_or_else(|_| format!("{{\"error\":true,\"message\":\"{self}\"}}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_spec() {
        assert_eq!(AppError::Auth("x".into()).exit_code(), 1);
        assert_eq!(AppError::InvalidInput("x".into()).exit_code(), 2);
        assert_eq!(AppError::Conflict("x".into()).exit_code(), 3);
        assert_eq!(AppError::NotFound("x".into()).exit_code(), 4);
        assert_eq!(AppError::RateLimited.exit_code(), 5);
        assert_eq!(AppError::ServerError("x".into()).exit_code(), 6);
        assert_eq!(AppError::Network("x".into()).exit_code(), 7);
        assert_eq!(AppError::Config("x".into()).exit_code(), 8);
    }

    #[test]
    fn to_json_contains_error_field() {
        let err = AppError::NotFound("Project 'foo' not found. Available: Alpha, Beta".into());
        let json: serde_json::Value = serde_json::from_str(&err.to_json()).unwrap();
        assert_eq!(json["error"], true);
        assert_eq!(json["code"], 4);
        assert!(json["message"].as_str().unwrap().contains("foo"));
    }

    #[test]
    fn suggestion_for_conflict() {
        let err = AppError::Conflict("A timer is already running.".into());
        assert_eq!(err.suggestion(), Some("keito time stop"));
    }

    #[test]
    fn suggestion_for_not_found_project() {
        let err = AppError::NotFound("Project 'foo' not found.".into());
        assert_eq!(err.suggestion(), Some("keito projects list --json"));
    }

    #[test]
    fn suggestion_for_auth() {
        let err = AppError::Auth("invalid key".into());
        assert_eq!(
            err.suggestion(),
            Some("Set KEITO_API_KEY env var or run 'keito auth login'")
        );
    }

    #[test]
    fn to_json_includes_suggestion() {
        let err = AppError::Conflict("A timer is already running.".into());
        let json: serde_json::Value = serde_json::from_str(&err.to_json()).unwrap();
        assert_eq!(json["suggestion"], "keito time stop");
    }

    #[test]
    fn to_json_includes_details() {
        let err = AppError::NotFound("Project 'foo' not found. Available: Alpha, Beta".into());
        let json: serde_json::Value = serde_json::from_str(&err.to_json()).unwrap();
        let available = json["details"]["available"].as_array().unwrap();
        assert_eq!(available.len(), 2);
        assert_eq!(available[0], "Alpha");
        assert_eq!(available[1], "Beta");
    }
}
