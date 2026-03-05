use crate::cli::GlobalFlags;
use crate::config::AppConfig;
use crate::error::AppError;

const KEYRING_SERVICE: &str = "keito-cli";
const KEYRING_USER: &str = "api_key";

#[derive(Debug)]
pub struct ResolvedAuth {
    pub api_key: String,
    pub workspace_id: String,
    pub api_key_source: String,
}

impl ResolvedAuth {
    /// Resolve credentials from: CLI flag > env var > keyring > config
    pub fn resolve(global: &GlobalFlags) -> Result<Self, AppError> {
        let config = AppConfig::load()?;

        // API key: env > keyring
        let (api_key, api_key_source) = Self::resolve_api_key()?;

        // Workspace: CLI flag > env > config
        let workspace_id: String = if let Some(ref ws) = global.workspace {
            ws.clone()
        } else if let Ok(ws) = std::env::var("KEITO_WORKSPACE_ID") {
            ws
        } else if let Some(ref ws) = config.workspace_id {
            ws.clone()
        } else {
            return Err(AppError::Config(
                "No workspace ID configured. Set via --workspace, KEITO_WORKSPACE_ID env var, or run 'keito auth login'".into(),
            ));
        };

        Ok(ResolvedAuth {
            api_key,
            workspace_id,
            api_key_source,
        })
    }

    fn resolve_api_key() -> Result<(String, String), AppError> {
        // 1. Environment variable
        if let Ok(key) = std::env::var("KEITO_API_KEY") {
            if !key.is_empty() {
                return Ok((key, "environment variable".into()));
            }
        }

        // 2. OS keyring
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            if let Ok(key) = entry.get_password() {
                if !key.is_empty() {
                    return Ok((key, "keyring".into()));
                }
            }
        }

        Err(AppError::Auth(
            "No API key found. Set KEITO_API_KEY env var or run 'keito auth login'".into(),
        ))
    }

    pub fn store_api_key(api_key: &str) -> Result<(), AppError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| {
            AppError::Config(format!(
                "Keyring unavailable: {e}. Use KEITO_API_KEY env var instead."
            ))
        })?;
        entry.set_password(api_key).map_err(|e| {
            AppError::Config(format!(
                "Failed to store key in keyring: {e}. Use KEITO_API_KEY env var instead."
            ))
        })?;
        Ok(())
    }

    pub fn delete_api_key() -> Result<(), AppError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .map_err(|e| AppError::Config(format!("Keyring unavailable: {e}")))?;
        entry
            .delete_credential()
            .map_err(|e| AppError::Config(format!("Failed to delete key from keyring: {e}")))?;
        Ok(())
    }
}
