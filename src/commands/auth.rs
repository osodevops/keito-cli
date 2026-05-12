use colored::Colorize;
use dialoguer::{Confirm, Input, Password};
use std::io::IsTerminal;

use crate::api::KeitorClient;
use crate::cli::auth::{AuthCommand, AuthSubcommand};
use crate::cli::GlobalFlags;
use crate::commands::skill;
use crate::config::{AppConfig, ResolvedAuth};
use crate::error::AppError;
use crate::output::{self, OutputMode};

pub async fn run(cmd: AuthCommand, global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    match cmd.command {
        AuthSubcommand::Login => login(global, mode).await,
        AuthSubcommand::Logout => logout(global, mode).await,
        AuthSubcommand::Status => status(global, mode).await,
        AuthSubcommand::Whoami => whoami(global, mode).await,
    }
}

async fn login(global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    let api_key = Password::new()
        .with_prompt("Enter your Keito API key")
        .interact()
        .map_err(|e| AppError::Config(format!("Input error: {e}")))?;

    if api_key.is_empty() {
        return Err(AppError::InvalidInput("API key cannot be empty".into()));
    }

    let account_id = resolve_login_account_id(global)?;

    // Validate the key and account ID before writing credentials.
    let mut config = AppConfig::load()?;
    let temp_auth = ResolvedAuth {
        api_key: api_key.clone(),
        workspace_id: account_id,
        api_key_source: "login".into(),
    };

    let client = KeitorClient::new(&temp_auth, &config.api_base_url())?;
    let me = client.get_me().await.map_err(|e| match e {
        AppError::Auth(_) => AppError::Auth("Invalid API key or account ID".into()),
        other => other,
    })?;

    // Store the long-lived API key in the config file. Agents can also use
    // KEITO_API_KEY for stateless execution.
    config.api_key = Some(api_key.clone());
    config.account_id = Some(me.company.id.clone());
    config.workspace_id = Some(me.company.id.clone());
    config.save()?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "authenticated",
            "user": me.display_name(),
            "company": me.company.name.clone(),
            "account_id": me.company.id.clone(),
            "workspace_id": me.company.id.clone(),
            "api_key_source": "config file",
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!(
            "{} Logged in as {} ({})",
            "Success!".green().bold(),
            me.display_name(),
            me.company.name
        );
        println!(
            "Credentials saved to {}.",
            AppConfig::config_path()?.display()
        );
        maybe_offer_skill_install(global, mode).await?;
    }

    Ok(())
}

async fn maybe_offer_skill_install(global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    if mode != OutputMode::Table || global.quiet || !std::io::stdin().is_terminal() {
        return Ok(());
    }

    let install = Confirm::new()
        .with_prompt("Install the Keito agent skill for Claude Code / Codex?")
        .default(true)
        .interact()
        .map_err(|e| AppError::Config(format!("Input error: {e}")))?;

    if install {
        skill::install_defaults(global, mode).await?;
    } else {
        println!("You can install it later with: keito skill install");
    }

    Ok(())
}

fn resolve_login_account_id(global: &GlobalFlags) -> Result<String, AppError> {
    if let Some(id) = global.workspace.as_deref().filter(|id| !id.is_empty()) {
        return Ok(id.to_string());
    }

    if let Some(id) = std::env::var("KEITO_ACCOUNT_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .or_else(|| {
            std::env::var("KEITO_WORKSPACE_ID")
                .ok()
                .filter(|id| !id.is_empty())
        })
    {
        return Ok(id);
    }

    Input::<String>::new()
        .with_prompt(
            "Enter your Keito account/company ID (Settings > API & Developers > Company ID)",
        )
        .interact_text()
        .map_err(|e| AppError::Config(format!("Input error: {e}")))
        .and_then(|id| {
            let id = id.trim().to_string();
            if id.is_empty() {
                Err(AppError::InvalidInput("Account ID cannot be empty".into()))
            } else {
                Ok(id)
            }
        })
}

async fn logout(_global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    let mut config = AppConfig::load()?;
    config.api_key = None;
    config.account_id = None;
    config.workspace_id = None;
    config.save()?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "logged_out",
            "config_credentials_removed": true,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!("Credentials removed from config.");
    }

    Ok(())
}

async fn status(global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    match ResolvedAuth::resolve(global) {
        Ok(auth) => {
            let config = AppConfig::load()?;
            let client = KeitorClient::new(&auth, &config.api_base_url())?;
            let valid = client.get_me().await.is_ok();

            if mode == OutputMode::Json {
                let out = serde_json::json!({
                    "authenticated": true,
                    "api_key_source": auth.api_key_source,
                    "account_id": auth.workspace_id.clone(),
                    "workspace_id": auth.workspace_id.clone(),
                    "api_key_valid": valid,
                });
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
            } else {
                println!("Authenticated: yes");
                println!("API key source: {}", auth.api_key_source);
                println!("Account ID: {}", auth.workspace_id);
                println!(
                    "API key valid: {}",
                    if valid {
                        "yes".green().to_string()
                    } else {
                        "no".red().to_string()
                    }
                );
            }
        }
        Err(_) => {
            if mode == OutputMode::Json {
                println!(r#"{{"authenticated": false}}"#);
            } else {
                println!(
                    "Not authenticated. Run 'keito auth login' or set KEITO_API_KEY and KEITO_ACCOUNT_ID."
                );
            }
        }
    }
    Ok(())
}

async fn whoami(global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;
    let me = client.get_me().await?;

    output::render_single(&me, mode, global.quiet)
}
