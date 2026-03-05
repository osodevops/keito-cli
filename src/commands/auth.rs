use colored::Colorize;
use dialoguer::Password;

use crate::api::KeitorClient;
use crate::cli::auth::{AuthCommand, AuthSubcommand};
use crate::cli::GlobalFlags;
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

async fn login(_global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    let api_key = Password::new()
        .with_prompt("Enter your Keito API key")
        .interact()
        .map_err(|e| AppError::Config(format!("Input error: {e}")))?;

    if api_key.is_empty() {
        return Err(AppError::InvalidInput("API key cannot be empty".into()));
    }

    // Validate the key by calling /me
    let mut config = AppConfig::load()?;
    let temp_auth = ResolvedAuth {
        api_key: api_key.clone(),
        workspace_id: "temp".into(),
        api_key_source: "login".into(),
    };

    // We need a workspace ID from the API response
    let client = KeitorClient::new(&temp_auth, &config.api_base_url())?;
    let me = client.get_me().await.map_err(|e| match e {
        AppError::Auth(_) => AppError::Auth("Invalid API key".into()),
        other => other,
    })?;

    // Store credentials
    if let Err(e) = ResolvedAuth::store_api_key(&api_key) {
        if mode != OutputMode::Json {
            eprintln!(
                "{} Could not store key in keyring: {e}. Use KEITO_API_KEY env var instead.",
                "warning:".yellow().bold()
            );
        }
    }

    // Save workspace ID to config
    config.workspace_id = Some(me.company.id.clone());
    config.save()?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "authenticated",
            "user": me.user.name,
            "company": me.company.name,
            "workspace_id": me.company.id,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!(
            "{} Logged in as {} ({})",
            "Success!".green().bold(),
            me.user.name,
            me.company.name
        );
    }

    Ok(())
}

async fn logout(_global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    ResolvedAuth::delete_api_key()?;

    if mode == OutputMode::Json {
        println!(r#"{{"status": "logged_out"}}"#);
    } else {
        println!("Credentials removed from keyring.");
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
                    "workspace_id": auth.workspace_id,
                    "api_key_valid": valid,
                });
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
            } else {
                println!("Authenticated: yes");
                println!("API key source: {}", auth.api_key_source);
                println!("Workspace ID: {}", auth.workspace_id);
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
                println!("Not authenticated. Run 'keito auth login' or set KEITO_API_KEY.");
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
