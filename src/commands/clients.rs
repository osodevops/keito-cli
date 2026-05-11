use crate::api::models::CreateClientRequest;
use crate::api::KeitorClient;
use crate::cli::clients::{ClientsCommand, ClientsSubcommand};
use crate::cli::GlobalFlags;
use crate::config::{AppConfig, ResolvedAuth};
use crate::error::AppError;
use crate::output::{self, OutputMode};

pub async fn run(
    cmd: ClientsCommand,
    global: &GlobalFlags,
    mode: OutputMode,
) -> Result<(), AppError> {
    match cmd.command {
        ClientsSubcommand::List { limit } => list(global, mode, limit).await,
        ClientsSubcommand::Create {
            name,
            currency,
            address,
        } => create(global, mode, name, currency, address).await,
    }
}

async fn list(
    global: &GlobalFlags,
    mode: OutputMode,
    limit: Option<usize>,
) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let mut clients = client.list_clients().await?;
    if let Some(limit) = limit {
        clients.truncate(limit);
    }

    output::render(&clients, mode, global.quiet)
}

async fn create(
    global: &GlobalFlags,
    mode: OutputMode,
    name: String,
    currency: Option<String>,
    address: Option<String>,
) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::InvalidInput("client name cannot be empty".into()));
    }

    let created = client
        .create_client(&CreateClientRequest {
            name,
            address,
            currency,
        })
        .await?;

    output::render_single(&created, mode, global.quiet)
}
