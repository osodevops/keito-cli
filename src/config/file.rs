use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::AppError;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub default_output: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
}

impl AppConfig {
    pub fn config_dir() -> Result<PathBuf, AppError> {
        let dir = dirs::config_dir()
            .ok_or_else(|| AppError::Config("Could not determine config directory".into()))?
            .join("keito");
        Ok(dir)
    }

    pub fn config_path() -> Result<PathBuf, AppError> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Self, AppError> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| AppError::Config(format!("Failed to read config: {e}")))?;
        let config: AppConfig = toml::from_str(&contents)
            .map_err(|e| AppError::Config(format!("Invalid config TOML: {e}")))?;
        Ok(config)
    }

    pub fn save(&self) -> Result<(), AppError> {
        let dir = Self::config_dir()?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::Config(format!("Failed to create config dir: {e}")))?;
        let path = Self::config_path()?;
        let contents = toml::to_string_pretty(self)
            .map_err(|e| AppError::Config(format!("Failed to serialize config: {e}")))?;
        std::fs::write(&path, contents)
            .map_err(|e| AppError::Config(format!("Failed to write config: {e}")))?;
        Ok(())
    }

    pub fn api_base_url(&self) -> String {
        self.api_url
            .clone()
            .unwrap_or_else(|| "https://app.keito.io".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config_toml() {
        let toml_str = r#"
workspace_id = "ws_123"
default_output = "json"
timezone = "Europe/London"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.workspace_id.as_deref(), Some("ws_123"));
        assert_eq!(config.default_output.as_deref(), Some("json"));
        assert_eq!(config.timezone.as_deref(), Some("Europe/London"));
    }

    #[test]
    fn empty_config_uses_defaults() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert!(config.workspace_id.is_none());
        assert_eq!(config.api_base_url(), "https://app.keito.io");
    }
}
