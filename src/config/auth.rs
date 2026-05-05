use crate::cli::GlobalFlags;
use crate::config::AppConfig;
use crate::error::AppError;

#[derive(Debug)]
pub struct ResolvedAuth {
    pub api_key: String,
    pub workspace_id: String,
    pub api_key_source: String,
}

impl ResolvedAuth {
    /// Resolve credentials from env vars and the config file.
    pub fn resolve(global: &GlobalFlags) -> Result<Self, AppError> {
        let config = AppConfig::load()?;

        // API key: env > config
        let (api_key, api_key_source) = Self::resolve_api_key(&config)?;

        // Account/workspace ID: CLI flag > env > config.
        // Production v2 sends this value as the Keito-Account-Id header.
        let workspace_id: String = if let Some(ref id) = global.workspace {
            id.clone()
        } else if let Some(id) =
            non_empty_env("KEITO_ACCOUNT_ID").or_else(|| non_empty_env("KEITO_WORKSPACE_ID"))
        {
            id
        } else if let Some(id) = config.resolved_account_id() {
            id.to_string()
        } else {
            return Err(AppError::Config(
                "No account ID configured. Set via --workspace, KEITO_ACCOUNT_ID, KEITO_WORKSPACE_ID, account_id in config, or run 'keito auth login'. Find it in Keito under Settings > API & Developers > Company ID".into(),
            ));
        };

        Ok(ResolvedAuth {
            api_key,
            workspace_id,
            api_key_source,
        })
    }

    fn resolve_api_key(config: &AppConfig) -> Result<(String, String), AppError> {
        // 1. Environment variable
        if let Some(key) = non_empty_env("KEITO_API_KEY") {
            return Ok((key, "environment variable".into()));
        }

        // 2. Config file
        if let Some(key) = config.api_key.as_deref().filter(|key| !key.is_empty()) {
            return Ok((key.to_string(), "config file".into()));
        }

        Err(AppError::Auth(
            "No API key found. Set KEITO_API_KEY, configure api_key in config.toml, or run 'keito auth login'".into(),
        ))
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
