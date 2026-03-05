use chrono::{Local, Utc};
use colored::Colorize;

use crate::api::models::{CreateTimeEntryRequest, UpdateTimeEntryRequest};
use crate::api::KeitorClient;
use crate::cli::time::{TimeCommand, TimeSubcommand};
use crate::cli::GlobalFlags;
use crate::config::{AppConfig, ResolvedAuth};
use crate::error::AppError;
use crate::output::{self, OutputMode};
use crate::types::{format_duration, parse_duration, resolve_name_to_id};

pub async fn run(cmd: TimeCommand, global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    match cmd.command {
        TimeSubcommand::Start {
            project,
            task,
            notes,
            billable,
        } => start(global, mode, &project, &task, notes, billable).await,
        TimeSubcommand::Stop { notes, discard } => stop(global, mode, notes, discard).await,
        TimeSubcommand::Log {
            project,
            task,
            duration,
            date,
            notes,
            billable,
        } => {
            log_entry(
                global, mode, &project, &task, &duration, date, notes, billable,
            )
            .await
        }
        TimeSubcommand::List {
            from,
            to,
            project,
            task,
            limit,
            page,
        } => list(global, mode, from, to, project, task, limit, page).await,
        TimeSubcommand::Running => running(global, mode).await,
    }
}

async fn start(
    global: &GlobalFlags,
    mode: OutputMode,
    project_query: &str,
    task_query: &str,
    notes: Option<String>,
    billable: Option<bool>,
) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    // Check for already-running timer
    let running = client.list_time_entries("is_running=true").await?;
    if !running.is_empty() {
        return Err(AppError::Conflict(
            "A timer is already running. Stop it first with 'keito time stop'.".into(),
        ));
    }

    // Resolve project
    let projects = client.list_projects().await?;
    let project_items: Vec<(String, String, Option<String>)> = projects
        .iter()
        .map(|p| (p.id.clone(), p.name.clone(), p.code.clone()))
        .collect();
    let project_id = resolve_name_to_id(project_query, &project_items, "Project")?.to_string();

    // Resolve task
    let tasks = client.list_tasks().await?;
    let task_items: Vec<(String, String, Option<String>)> = tasks
        .iter()
        .map(|t| (t.id.clone(), t.name.clone(), None))
        .collect();
    let task_id = resolve_name_to_id(task_query, &task_items, "Task")?.to_string();

    let entry = client
        .create_time_entry(&CreateTimeEntryRequest {
            project_id,
            task_id,
            date: Some(Local::now().format("%Y-%m-%d").to_string()),
            hours: None,
            notes,
            is_billable: billable,
            is_running: true,
        })
        .await?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "started",
            "entry_id": entry.id,
            "project": entry.project_name,
            "task": entry.task_name,
            "started_at": entry.timer_started_at,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!(
            "{} Timer started for {} / {}",
            "Started!".green().bold(),
            entry.project_name.as_deref().unwrap_or("?"),
            entry.task_name.as_deref().unwrap_or("?"),
        );
    }

    Ok(())
}

async fn stop(
    global: &GlobalFlags,
    mode: OutputMode,
    notes: Option<String>,
    discard: bool,
) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let running = client.list_time_entries("is_running=true").await?;

    let timer = running
        .into_iter()
        .next()
        .ok_or_else(|| AppError::NotFound("No running timer found.".into()))?;

    if discard {
        client.delete_time_entry(&timer.id).await?;

        if mode == OutputMode::Json {
            let out = serde_json::json!({
                "status": "discarded",
                "entry_id": timer.id,
                "project": timer.project_name,
                "task": timer.task_name,
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        } else {
            println!(
                "{} Timer discarded for {} / {}",
                "Discarded!".yellow().bold(),
                timer.project_name.as_deref().unwrap_or("?"),
                timer.task_name.as_deref().unwrap_or("?"),
            );
        }

        return Ok(());
    }

    let elapsed_hours = timer.timer_started_at.map(|started| {
        let elapsed = Utc::now() - started;
        elapsed.num_seconds() as f64 / 3600.0
    });

    let entry = client
        .update_time_entry(
            &timer.id,
            &UpdateTimeEntryRequest {
                is_running: Some(false),
                notes,
                hours: elapsed_hours,
            },
        )
        .await?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "stopped",
            "entry_id": entry.id,
            "project": entry.project_name,
            "task": entry.task_name,
            "duration_hours": entry.hours,
            "duration": entry.hours.map(format_duration),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!(
            "{} Timer stopped — {} for {} / {}",
            "Stopped!".green().bold(),
            entry.hours.map(format_duration).unwrap_or_default(),
            entry.project_name.as_deref().unwrap_or("?"),
            entry.task_name.as_deref().unwrap_or("?"),
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn log_entry(
    global: &GlobalFlags,
    mode: OutputMode,
    project_query: &str,
    task_query: &str,
    duration_str: &str,
    date: Option<String>,
    notes: Option<String>,
    billable: Option<bool>,
) -> Result<(), AppError> {
    let hours = parse_duration(duration_str)?;

    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    // Resolve project
    let projects = client.list_projects().await?;
    let project_items: Vec<(String, String, Option<String>)> = projects
        .iter()
        .map(|p| (p.id.clone(), p.name.clone(), p.code.clone()))
        .collect();
    let project_id = resolve_name_to_id(project_query, &project_items, "Project")?.to_string();

    // Resolve task
    let tasks = client.list_tasks().await?;
    let task_items: Vec<(String, String, Option<String>)> = tasks
        .iter()
        .map(|t| (t.id.clone(), t.name.clone(), None))
        .collect();
    let task_id = resolve_name_to_id(task_query, &task_items, "Task")?.to_string();

    let date_str = date.unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());

    let entry = client
        .create_time_entry(&CreateTimeEntryRequest {
            project_id,
            task_id,
            date: Some(date_str),
            hours: Some(hours),
            notes,
            is_billable: billable,
            is_running: false,
        })
        .await?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "logged",
            "entry_id": entry.id,
            "project": entry.project_name,
            "task": entry.task_name,
            "duration_hours": entry.hours,
            "duration": entry.hours.map(format_duration),
            "date": entry.date,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!(
            "{} Logged {} for {} / {}",
            "Logged!".green().bold(),
            entry.hours.map(format_duration).unwrap_or_default(),
            entry.project_name.as_deref().unwrap_or("?"),
            entry.task_name.as_deref().unwrap_or("?"),
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn list(
    global: &GlobalFlags,
    mode: OutputMode,
    from: Option<String>,
    to: Option<String>,
    project: Option<String>,
    task: Option<String>,
    limit: u32,
    _page: u32,
) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let mut params = vec![format!("per_page={limit}")];

    if let Some(ref from) = from {
        params.push(format!("from={from}"));
    }
    if let Some(ref to) = to {
        params.push(format!("to={to}"));
    }

    // Resolve project ID if provided
    if let Some(ref project_query) = project {
        let projects = client.list_projects().await?;
        let project_items: Vec<(String, String, Option<String>)> = projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.code.clone()))
            .collect();
        let project_id = resolve_name_to_id(project_query, &project_items, "Project")?;
        params.push(format!("project_id={project_id}"));
    }

    // Resolve task ID if provided
    if let Some(ref task_query) = task {
        let tasks = client.list_tasks().await?;
        let task_items: Vec<(String, String, Option<String>)> = tasks
            .iter()
            .map(|t| (t.id.clone(), t.name.clone(), None))
            .collect();
        let task_id = resolve_name_to_id(task_query, &task_items, "Task")?;
        params.push(format!("task_id={task_id}"));
    }

    let query = params.join("&");
    let entries = client.list_time_entries(&query).await?;

    output::render(&entries, mode, global.quiet)
}

async fn running(global: &GlobalFlags, mode: OutputMode) -> Result<(), AppError> {
    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let running = client.list_time_entries("is_running=true").await?;

    if running.is_empty() {
        if mode == OutputMode::Json {
            println!(r#"{{"running": false}}"#);
        } else if !global.quiet {
            println!("No timer running.");
        }
        return Ok(());
    }

    if mode == OutputMode::Json {
        let mut entries: Vec<serde_json::Value> = Vec::new();
        for entry in &running {
            let elapsed = entry.timer_started_at.map(|started| {
                let elapsed = Utc::now() - started;
                elapsed.num_seconds() as f64 / 3600.0
            });
            entries.push(serde_json::json!({
                "running": true,
                "entry_id": entry.id,
                "project": entry.project_name,
                "task": entry.task_name,
                "started_at": entry.timer_started_at,
                "elapsed_hours": elapsed,
                "elapsed": elapsed.map(format_duration),
            }));
        }
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else {
        for entry in &running {
            let elapsed = entry.timer_started_at.map(|started| {
                let elapsed = Utc::now() - started;
                format_duration(elapsed.num_seconds() as f64 / 3600.0)
            });
            println!(
                "{} {} / {} — {} elapsed",
                "Running:".green().bold(),
                entry.project_name.as_deref().unwrap_or("?"),
                entry.task_name.as_deref().unwrap_or("?"),
                elapsed.unwrap_or_else(|| "?".into()),
            );
        }
    }

    Ok(())
}
