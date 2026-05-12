use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::AppError;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
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

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| AppError::Config(format!("Failed to secure config dir: {e}")))?;
        }

        let path = Self::config_path()?;
        let contents = toml::to_string_pretty(self)
            .map_err(|e| AppError::Config(format!("Failed to serialize config: {e}")))?;
        std::fs::write(&path, contents)
            .map_err(|e| AppError::Config(format!("Failed to write config: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| AppError::Config(format!("Failed to secure config file: {e}")))?;
        }

        Ok(())
    }

    pub fn api_base_url(&self) -> String {
        if let Ok(api_url) = std::env::var("KEITO_API_URL") {
            let api_url = api_url.trim();
            if !api_url.is_empty() {
                return api_url.to_string();
            }
        }

        self.api_url
            .clone()
            .unwrap_or_else(|| "https://app.keito.ai".into())
    }

    pub fn resolved_account_id(&self) -> Option<&str> {
        self.account_id.as_deref().or(self.workspace_id.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static KEITO_API_URL_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: Option<&str>) -> Self {
            let previous = std::env::var_os(name);
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    #[test]
    fn parse_config_toml() {
        let toml_str = r#"
api_key = "kto_123"
account_id = "co_123"
workspace_id = "ws_123"
default_output = "json"
timezone = "Europe/London"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.api_key.as_deref(), Some("kto_123"));
        assert_eq!(config.account_id.as_deref(), Some("co_123"));
        assert_eq!(config.workspace_id.as_deref(), Some("ws_123"));
        assert_eq!(config.resolved_account_id(), Some("co_123"));
        assert_eq!(config.default_output.as_deref(), Some("json"));
        assert_eq!(config.timezone.as_deref(), Some("Europe/London"));
    }

    #[test]
    fn empty_config_uses_defaults() {
        let _guard = KEITO_API_URL_ENV_LOCK.lock().unwrap();
        let _env = EnvVarGuard::set("KEITO_API_URL", None);

        let config: AppConfig = toml::from_str("").unwrap();
        assert!(config.api_key.is_none());
        assert!(config.account_id.is_none());
        assert!(config.workspace_id.is_none());
        assert_eq!(config.api_base_url(), "https://app.keito.ai");
    }

    #[test]
    fn api_url_env_overrides_config() {
        let _guard = KEITO_API_URL_ENV_LOCK.lock().unwrap();
        let _env = EnvVarGuard::set("KEITO_API_URL", Some("http://localhost:3999"));

        let config: AppConfig = toml::from_str(r#"api_url = "https://app.keito.ai""#).unwrap();
        assert_eq!(config.api_base_url(), "http://localhost:3999");
    }

    #[test]
    fn workspace_id_is_account_id_fallback() {
        let config: AppConfig = toml::from_str(r#"workspace_id = "ws_legacy""#).unwrap();
        assert_eq!(config.resolved_account_id(), Some("ws_legacy"));
    }
}
