use crate::api::models::CreateProjectRequest;
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
        ProjectsSubcommand::List { limit, client } => list(global, mode, limit, client).await,
        ProjectsSubcommand::Create {
            name,
            client,
            code,
            notes,
            billable,
            tasks,
        } => create(global, mode, name, client, code, notes, billable, tasks).await,
        ProjectsSubcommand::Show { project } => show(global, mode, &project).await,
        ProjectsSubcommand::Tasks { limit } => tasks(global, mode, limit).await,
    }
}

async fn list(
    global: &GlobalFlags,
    mode: OutputMode,
    limit: Option<u32>,
    client_id: Option<String>,
) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let mut projects = client
        .list_projects_for_client(client_id.as_deref())
        .await?;
    if let Some(limit) = limit {
        projects.truncate(limit as usize);
    }

    output::render(&projects, mode, global.quiet)
}

#[allow(clippy::too_many_arguments)]
async fn create(
    global: &GlobalFlags,
    mode: OutputMode,
    name: String,
    client_id: String,
    code: Option<String>,
    notes: Option<String>,
    billable: Option<bool>,
    tasks: Vec<String>,
) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::InvalidInput(
            "project name cannot be empty".into(),
        ));
    }
    let client_id = client_id.trim().to_string();
    if client_id.is_empty() {
        return Err(AppError::InvalidInput("client ID cannot be empty".into()));
    }

    let created = client
        .create_project(&CreateProjectRequest {
            client_id,
            name,
            code: code.and_then(non_empty),
            notes: notes.and_then(non_empty),
            is_billable: billable,
            bill_by: billable.map(|value| if value { "PROJECT" } else { "NONE" }.to_string()),
            budget_by: Some("NONE".into()),
            task_ids: normalize_task_ids(tasks)?,
        })
        .await?;

    output::render_single(&created, mode, global.quiet)
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

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn normalize_task_ids(values: Vec<String>) -> Result<Option<Vec<String>>, AppError> {
    let mut task_ids = Vec::new();

    for value in values {
        let value = value.trim().to_string();
        if value.is_empty() {
            return Err(AppError::InvalidInput("task ID cannot be empty".into()));
        }
        if !task_ids.iter().any(|existing| existing == &value) {
            task_ids.push(value);
        }
    }

    if task_ids.is_empty() {
        Ok(None)
    } else {
        Ok(Some(task_ids))
    }
}
