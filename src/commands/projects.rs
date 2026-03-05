use crate::api::KeitorClient;
use crate::cli::projects::{ProjectsCommand, ProjectsSubcommand};
use crate::cli::GlobalFlags;
use crate::config::{AppConfig, ResolvedAuth};
use crate::error::AppError;
use crate::output::{self, OutputMode};
use crate::types::resolve_name_to_id;

pub async fn run(
    cmd: ProjectsCommand,
    global: &GlobalFlags,
    mode: OutputMode,
) -> Result<(), AppError> {
    match cmd.command {
        ProjectsSubcommand::List { limit } => list(global, mode, limit).await,
        ProjectsSubcommand::Show { project } => show(global, mode, &project).await,
        ProjectsSubcommand::Tasks { limit } => tasks(global, mode, limit).await,
    }
}

async fn list(global: &GlobalFlags, mode: OutputMode, limit: Option<u32>) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let mut projects = client.list_projects().await?;
    if let Some(limit) = limit {
        projects.truncate(limit as usize);
    }

    output::render(&projects, mode, global.quiet)
}

async fn show(global: &GlobalFlags, mode: OutputMode, query: &str) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let projects = client.list_projects().await?;
    let items: Vec<(String, String, Option<String>)> = projects
        .iter()
        .map(|p| (p.id.clone(), p.name.clone(), p.code.clone()))
        .collect();

    let matched_id = resolve_name_to_id(query, &items, "Project")?;
    let project = projects.into_iter().find(|p| p.id == matched_id).unwrap();

    output::render_single(&project, mode, global.quiet)
}

async fn tasks(global: &GlobalFlags, mode: OutputMode, limit: Option<u32>) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let mut tasks = client.list_tasks().await?;
    if let Some(limit) = limit {
        tasks.truncate(limit as usize);
    }

    output::render(&tasks, mode, global.quiet)
}
