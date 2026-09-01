use chrono::{DateTime, Local, Utc};
use colored::Colorize;
use serde_json::{Map, Value};

use crate::api::models::{
    CreateTimeEntryRequest, Project, Task, TimeEntry, UpdateTimeEntryRequest,
};
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
            duration_seconds,
            date,
            started_time,
            ended_time,
            notes,
            billable,
            source,
            metadata,
            session_id,
            agent_id,
            agent_type,
            skill,
        } => {
            log_entry(
                global,
                mode,
                &project,
                &task,
                duration,
                duration_seconds,
                date,
                started_time,
                ended_time,
                notes,
                billable,
                source,
                metadata,
                session_id,
                agent_id,
                agent_type,
                skill,
            )
            .await
        }
        TimeSubcommand::SessionRecord {
            project,
            task,
            session_id,
            duration_seconds,
            started_at,
            ended_at,
            date,
            notes,
            billable,
            source,
            metadata,
            agent_id,
            agent_type,
            skill,
        } => {
            session_record(
                global,
                mode,
                &project,
                &task,
                session_id,
                duration_seconds,
                started_at,
                ended_at,
                date,
                notes,
                billable,
                source,
                metadata,
                agent_id,
                agent_type,
                skill,
            )
            .await
        }
        TimeSubcommand::List {
            from,
            to,
            today,
            project,
            task,
            source,
            limit,
            page,
        } => {
            list(
                global, mode, from, to, today, project, task, source, limit, page,
            )
            .await
        }
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
    let tasks = tasks_for_resolved_project(&client, &projects, &project_id).await?;
    let task_items: Vec<(String, String, Option<String>)> = tasks
        .iter()
        .map(|t| (t.id.clone(), t.name.clone(), None))
        .collect();
    let task_id = resolve_name_to_id(task_query, &task_items, "Task")?.to_string();

    let entry = client
        .create_time_entry(&CreateTimeEntryRequest {
            project_id,
            task_id,
            spent_date: Local::now().format("%Y-%m-%d").to_string(),
            hours: None,
            notes,
            billable,
            is_running: true,
            started_time: None,
            ended_time: None,
            source: Some("cli".into()),
            metadata: None,
        })
        .await?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "started",
            "entry_id": entry.id,
            "project": entry.project_name(),
            "task": entry.task_name(),
            "spent_date": entry.spent_date,
            "billable": entry.billable,
            "source": entry.source,
            "started_at": entry.timer_started_at,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!(
            "{} Timer started for {} / {}",
            "Started!".green().bold(),
            entry.project_name().unwrap_or("?"),
            entry.task_name().unwrap_or("?"),
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
                "project": timer.project_name(),
                "task": timer.task_name(),
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        } else {
            println!(
                "{} Timer discarded for {} / {}",
                "Discarded!".yellow().bold(),
                timer.project_name().unwrap_or("?"),
                timer.task_name().unwrap_or("?"),
            );
        }

        return Ok(());
    }

    let entry = client.stop_time_entry(&timer.id, notes.as_deref()).await?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "stopped",
            "entry_id": entry.id,
            "project": entry.project_name(),
            "task": entry.task_name(),
            "duration_hours": entry.actual_hours(),
            "duration_seconds": entry.duration_seconds,
            "rounded_hours": entry.rounded_hours,
            "duration": entry.actual_hours().map(format_duration),
            "spent_date": entry.spent_date,
            "billable": entry.billable,
            "source": entry.source,
            "started_at": timer.timer_started_at,
            "stopped_at": entry.updated_at,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!(
            "{} Timer stopped — {} for {} / {}",
            "Stopped!".green().bold(),
            entry
                .actual_hours()
                .map(format_duration)
                .unwrap_or_default(),
            entry.project_name().unwrap_or("?"),
            entry.task_name().unwrap_or("?"),
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
    duration_str: Option<String>,
    duration_seconds: Option<u64>,
    date: Option<String>,
    started_time: Option<String>,
    ended_time: Option<String>,
    notes: Option<String>,
    billable: Option<bool>,
    source: String,
    metadata: Option<String>,
    session_id: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    skill: Option<String>,
) -> Result<(), AppError> {
    let hours = hours_from_duration_inputs(duration_str.as_deref(), duration_seconds)?;
    let metadata_duration_seconds =
        duration_seconds.unwrap_or_else(|| (hours * 3600.0).round() as u64);
    let source = normalize_source(&source)?;
    let metadata = prepare_agent_log_metadata(
        build_metadata(MetadataInput {
            metadata,
            session_id,
            agent_id,
            agent_type,
            skill,
        })?,
        &source,
        metadata_duration_seconds,
    )?;

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
    let tasks = tasks_for_resolved_project(&client, &projects, &project_id).await?;
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
            spent_date: date_str,
            hours: Some(hours),
            notes,
            billable,
            is_running: false,
            started_time,
            ended_time,
            source: Some(source),
            metadata,
        })
        .await?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "logged",
            "entry_id": entry.id,
            "project": entry.project_name(),
            "task": entry.task_name(),
            "duration_hours": entry.actual_hours(),
            "duration_seconds": entry.duration_seconds,
            "rounded_hours": entry.rounded_hours,
            "duration": entry.actual_hours().map(format_duration),
            "spent_date": entry.spent_date,
            "date": entry.spent_date,
            "billable": entry.billable,
            "source": entry.source,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!(
            "{} Logged {} for {} / {}",
            "Logged!".green().bold(),
            entry
                .actual_hours()
                .map(format_duration)
                .unwrap_or_default(),
            entry.project_name().unwrap_or("?"),
            entry.task_name().unwrap_or("?"),
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn session_record(
    global: &GlobalFlags,
    mode: OutputMode,
    project_query: &str,
    task_query: &str,
    session_id: String,
    duration_seconds: u64,
    started_at: Option<String>,
    ended_at: Option<String>,
    date: Option<String>,
    notes: Option<String>,
    billable: Option<bool>,
    source: String,
    metadata: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    skill: Option<String>,
) -> Result<(), AppError> {
    let hours = hours_from_seconds(duration_seconds)?;
    let source = normalize_source(&source)?;
    let spent_date = session_spent_date(date, started_at.as_deref())?;
    let started_time = started_at
        .as_deref()
        .map(local_time_from_rfc3339)
        .transpose()?;
    let ended_time = ended_at
        .as_deref()
        .map(local_time_from_rfc3339)
        .transpose()?;
    let metadata = build_session_metadata(
        MetadataInput {
            metadata,
            session_id: Some(session_id.clone()),
            agent_id,
            agent_type,
            skill,
        },
        duration_seconds,
        &source,
    )?;

    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let projects = client.list_projects().await?;
    let project_items: Vec<(String, String, Option<String>)> = projects
        .iter()
        .map(|p| (p.id.clone(), p.name.clone(), p.code.clone()))
        .collect();
    let project_id = resolve_name_to_id(project_query, &project_items, "Project")?.to_string();

    let tasks = tasks_for_resolved_project(&client, &projects, &project_id).await?;
    let task_items: Vec<(String, String, Option<String>)> = tasks
        .iter()
        .map(|t| (t.id.clone(), t.name.clone(), None))
        .collect();
    let task_id = resolve_name_to_id(task_query, &task_items, "Task")?.to_string();

    let query = format!("from={spent_date}&to={spent_date}&source={source}&per_page=200");
    let existing = client
        .list_time_entries(&query)
        .await?
        .into_iter()
        .find(|entry| entry_session_id(entry) == Some(session_id.as_str()));

    let (status, entry) = if let Some(existing) = existing {
        let entry = client
            .update_time_entry(
                &existing.id,
                &UpdateTimeEntryRequest {
                    project_id: Some(project_id),
                    task_id: Some(task_id),
                    spent_date: Some(spent_date.clone()),
                    notes,
                    hours: Some(hours),
                    billable,
                    started_time: started_time.clone(),
                    ended_time: ended_time.clone(),
                    metadata: Some(metadata.clone()),
                },
            )
            .await?;
        ("updated", entry)
    } else {
        let entry = client
            .create_time_entry(&CreateTimeEntryRequest {
                project_id,
                task_id,
                spent_date: spent_date.clone(),
                hours: Some(hours),
                notes,
                billable,
                is_running: false,
                started_time,
                ended_time,
                source: Some(source.clone()),
                metadata: Some(metadata.clone()),
            })
            .await?;
        ("created", entry)
    };

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": status,
            "entry_id": entry.id,
            "project": entry.project_name(),
            "task": entry.task_name(),
            "duration_hours": entry.actual_hours(),
            "duration_seconds": entry.duration_seconds,
            "rounded_hours": entry.rounded_hours,
            "duration": entry.actual_hours().map(format_duration),
            "spent_date": entry.spent_date,
            "billable": entry.billable,
            "source": entry.source,
            "session_id": session_id,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        let label = if status == "updated" {
            "Updated!"
        } else {
            "Recorded!"
        };
        println!(
            "{} {} for {} / {}",
            label.green().bold(),
            entry
                .actual_hours()
                .map(format_duration)
                .unwrap_or_default(),
            entry.project_name().unwrap_or("?"),
            entry.task_name().unwrap_or("?"),
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
    today: bool,
    project: Option<String>,
    task: Option<String>,
    source: Option<String>,
    limit: u32,
    page: u32,
) -> Result<(), AppError> {
    if !(1..=2000).contains(&limit) {
        return Err(AppError::InvalidInput(
            "--limit must be between 1 and 2000".into(),
        ));
    }
    if page == 0 {
        return Err(AppError::InvalidInput(
            "--page must be greater than zero".into(),
        ));
    }

    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let mut params = vec![format!("per_page={limit}"), format!("page={page}")];
    let (from, to) = if today {
        if from.is_some() || to.is_some() {
            return Err(AppError::InvalidInput(
                "--today cannot be combined with --from or --to".into(),
            ));
        }
        let today = Local::now().format("%Y-%m-%d").to_string();
        (Some(today.clone()), Some(today))
    } else {
        (from, to)
    };

    if let Some(ref from) = from {
        params.push(format!("from={from}"));
    }
    if let Some(ref to) = to {
        params.push(format!("to={to}"));
    }

    if let Some(source) = source {
        params.push(format!("source={}", normalize_source(&source)?));
    }

    let mut project_tasks = None;

    // Resolve project ID if provided
    if let Some(ref project_query) = project {
        let projects = client.list_projects().await?;
        let project_items: Vec<(String, String, Option<String>)> = projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.code.clone()))
            .collect();
        let project_id = resolve_name_to_id(project_query, &project_items, "Project")?.to_string();
        project_tasks = Some(tasks_for_resolved_project(&client, &projects, &project_id).await?);
        params.push(format!("project_id={project_id}"));
    }

    // Resolve task ID if provided
    if let Some(ref task_query) = task {
        let tasks = match project_tasks {
            Some(tasks) => tasks,
            None => client.list_tasks().await?,
        };
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
        // Production enforces one running timer per user. Keep the JSON shape
        // stable between the running and not-running states by returning one
        // object in both cases.
        let entry = &running[0];
        let elapsed = entry.timer_started_at.map(|started| {
            let elapsed = Utc::now() - started;
            elapsed.num_seconds() as f64 / 3600.0
        });
        let out = serde_json::json!({
            "running": true,
            "entry_id": entry.id,
            "project": entry.project_name(),
            "task": entry.task_name(),
            "started_at": entry.timer_started_at,
            "spent_date": entry.spent_date,
            "billable": entry.billable,
            "source": entry.source,
            "elapsed_hours": elapsed,
            "elapsed": elapsed.map(format_duration),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        for entry in &running {
            let elapsed = entry.timer_started_at.map(|started| {
                let elapsed = Utc::now() - started;
                format_duration(elapsed.num_seconds() as f64 / 3600.0)
            });
            println!(
                "{} {} / {} — {} elapsed",
                "Running:".green().bold(),
                entry.project_name().unwrap_or("?"),
                entry.task_name().unwrap_or("?"),
                elapsed.unwrap_or_else(|| "?".into()),
            );
        }
    }

    Ok(())
}

struct MetadataInput {
    metadata: Option<String>,
    session_id: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    skill: Option<String>,
}

fn normalize_source(source: &str) -> Result<String, AppError> {
    let normalized = source.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "web" | "cli" | "api" | "agent" | "calendar" | "desktop" | "mobile"
        | "integration" => Ok(normalized),
        _ => Err(AppError::InvalidInput(format!(
            "source must be one of: web, cli, api, agent, calendar, desktop, mobile, integration (got '{source}')"
        ))),
    }
}

async fn tasks_for_resolved_project(
    client: &KeitorClient,
    projects: &[Project],
    project_id: &str,
) -> Result<Vec<Task>, AppError> {
    if let Some(tasks) = projects
        .iter()
        .find(|project| project.id == project_id)
        .and_then(|project| project.tasks.clone())
    {
        return Ok(tasks);
    }

    client.list_tasks_for_project(Some(project_id)).await
}

fn hours_from_duration_inputs(
    duration: Option<&str>,
    duration_seconds: Option<u64>,
) -> Result<f64, AppError> {
    match (duration, duration_seconds) {
        (Some(duration), None) => parse_duration(duration),
        (None, Some(seconds)) => hours_from_seconds(seconds),
        (None, None) => Err(AppError::InvalidInput(
            "provide either --duration or --duration-seconds".into(),
        )),
        (Some(_), Some(_)) => Err(AppError::InvalidInput(
            "--duration and --duration-seconds cannot be used together".into(),
        )),
    }
}

fn hours_from_seconds(seconds: u64) -> Result<f64, AppError> {
    if seconds == 0 {
        return Err(AppError::InvalidInput(
            "--duration-seconds must be greater than zero".into(),
        ));
    }
    Ok(((seconds as f64 / 3600.0) * 100.0).round() / 100.0)
}

fn build_metadata(input: MetadataInput) -> Result<Option<Value>, AppError> {
    let mut map = match input.metadata {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(AppError::InvalidInput("--metadata cannot be empty".into()));
            }
            match serde_json::from_str::<Value>(trimmed).map_err(|err| {
                AppError::InvalidInput(format!("--metadata must be valid JSON: {err}"))
            })? {
                Value::Object(map) => map,
                _ => {
                    return Err(AppError::InvalidInput(
                        "--metadata must be a JSON object".into(),
                    ))
                }
            }
        }
        None => Map::new(),
    };

    insert_string_metadata(&mut map, "session_id", input.session_id);
    insert_string_metadata(&mut map, "agent_id", input.agent_id);
    insert_string_metadata(&mut map, "agent_type", input.agent_type);
    insert_string_metadata(&mut map, "skill", input.skill);

    if map.is_empty() {
        return Ok(None);
    }

    let value = Value::Object(map);
    let size = serde_json::to_string(&value)
        .map_err(|err| AppError::InvalidInput(format!("failed to serialize metadata: {err}")))?
        .len();
    if size > 4096 {
        return Err(AppError::InvalidInput(
            "--metadata payload must be 4KB or smaller".into(),
        ));
    }

    Ok(Some(value))
}

fn build_session_metadata(
    input: MetadataInput,
    duration_seconds: u64,
    source: &str,
) -> Result<Value, AppError> {
    let mut metadata = match build_metadata(input)? {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    metadata.insert(
        "duration_seconds".into(),
        Value::Number(duration_seconds.into()),
    );
    if source == "agent" {
        match metadata.get("skill") {
            Some(Value::String(skill)) if skill == "keito-time-track" => {}
            Some(_) => {
                return Err(AppError::InvalidInput(
                    "source=agent session records require metadata.skill=keito-time-track".into(),
                ))
            }
            None => {
                metadata.insert("skill".into(), Value::String("keito-time-track".into()));
            }
        }
    }

    let value = Value::Object(metadata);
    let size = serde_json::to_string(&value)
        .map_err(|err| AppError::InvalidInput(format!("failed to serialize metadata: {err}")))?
        .len();
    if size > 4096 {
        return Err(AppError::InvalidInput(
            "--metadata payload must be 4KB or smaller".into(),
        ));
    }
    Ok(value)
}

fn prepare_agent_log_metadata(
    metadata: Option<Value>,
    source: &str,
    duration_seconds: u64,
) -> Result<Option<Value>, AppError> {
    if source != "agent" {
        return Ok(metadata);
    }

    let mut map = match metadata {
        Some(Value::Object(map)) => map,
        other => return Ok(other),
    };
    if !map.contains_key("session_id") {
        // Strict public-build metadata is a separate valid source=agent shape;
        // leave it untouched for production to validate against the target user.
        return Ok(Some(Value::Object(map)));
    }

    match map.get("skill") {
        Some(Value::String(skill)) if skill == "keito-time-track" => {}
        Some(_) => {
            return Err(AppError::InvalidInput(
                "source=agent lifecycle metadata requires skill=keito-time-track".into(),
            ))
        }
        None => {
            map.insert("skill".into(), Value::String("keito-time-track".into()));
        }
    }
    map.insert(
        "duration_seconds".into(),
        Value::Number(duration_seconds.into()),
    );
    let value = Value::Object(map);
    let size = serde_json::to_string(&value)
        .map_err(|err| AppError::InvalidInput(format!("failed to serialize metadata: {err}")))?
        .len();
    if size > 4096 {
        return Err(AppError::InvalidInput(
            "--metadata payload must be 4KB or smaller".into(),
        ));
    }
    Ok(Some(value))
}

fn insert_string_metadata(map: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        let value = value.trim();
        if !value.is_empty() {
            map.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
}

fn session_spent_date(date: Option<String>, started_at: Option<&str>) -> Result<String, AppError> {
    if let Some(date) = date {
        return Ok(date);
    }
    if let Some(started_at) = started_at {
        return local_date_from_rfc3339(started_at);
    }
    Ok(Local::now().format("%Y-%m-%d").to_string())
}

fn local_date_from_rfc3339(input: &str) -> Result<String, AppError> {
    Ok(parse_rfc3339(input)?
        .with_timezone(&Local)
        .format("%Y-%m-%d")
        .to_string())
}

fn local_time_from_rfc3339(input: &str) -> Result<String, AppError> {
    Ok(parse_rfc3339(input)?
        .with_timezone(&Local)
        .format("%H:%M")
        .to_string())
}

fn parse_rfc3339(input: &str) -> Result<DateTime<chrono::FixedOffset>, AppError> {
    DateTime::parse_from_rfc3339(input).map_err(|err| {
        AppError::InvalidInput(format!("invalid RFC3339 timestamp '{input}': {err}"))
    })
}

fn entry_session_id(entry: &TimeEntry) -> Option<&str> {
    entry.metadata.as_ref()?.get("session_id")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_normalized_and_validated() {
        assert_eq!(normalize_source("Agent").unwrap(), "agent");
        assert_eq!(normalize_source("desktop").unwrap(), "desktop");
        assert_eq!(normalize_source("Calendar").unwrap(), "calendar");
        assert_eq!(normalize_source("mobile").unwrap(), "mobile");
        assert_eq!(normalize_source("integration").unwrap(), "integration");
        assert!(normalize_source("unknown").is_err());
    }

    #[test]
    fn duration_seconds_rounds_to_api_precision() {
        assert_eq!(hours_from_seconds(5400).unwrap(), 1.5);
        assert_eq!(hours_from_seconds(90).unwrap(), 0.03);
        assert!(hours_from_seconds(0).is_err());
    }

    #[test]
    fn metadata_must_be_an_object_and_agent_fields_override() {
        let metadata = build_metadata(MetadataInput {
            metadata: Some(r#"{"session_id":"old","env":"local"}"#.into()),
            session_id: Some("new".into()),
            agent_id: Some("codex".into()),
            agent_type: None,
            skill: Some("keito-agent".into()),
        })
        .unwrap()
        .unwrap();

        assert_eq!(metadata["session_id"], "new");
        assert_eq!(metadata["env"], "local");
        assert_eq!(metadata["agent_id"], "codex");
        assert_eq!(metadata["skill"], "keito-agent");
        assert!(build_metadata(MetadataInput {
            metadata: Some("[]".into()),
            session_id: None,
            agent_id: None,
            agent_type: None,
            skill: None,
        })
        .is_err());
    }

    #[test]
    fn session_metadata_matches_production_agent_lifecycle_contract() {
        let metadata = build_session_metadata(
            MetadataInput {
                metadata: None,
                session_id: Some("session-123".into()),
                agent_id: Some("codex".into()),
                agent_type: Some("codex".into()),
                skill: None,
            },
            5400,
            "agent",
        )
        .unwrap();

        assert_eq!(metadata["skill"], "keito-time-track");
        assert_eq!(metadata["session_id"], "session-123");
        assert_eq!(metadata["duration_seconds"], 5400);

        assert!(build_session_metadata(
            MetadataInput {
                metadata: None,
                session_id: Some("session-123".into()),
                agent_id: None,
                agent_type: None,
                skill: Some("another-skill".into()),
            },
            5400,
            "agent",
        )
        .is_err());
    }

    #[test]
    fn agent_log_lifecycle_metadata_gets_skill_and_duration() {
        let metadata = prepare_agent_log_metadata(
            Some(serde_json::json!({"session_id": "session-123"})),
            "agent",
            900,
        )
        .unwrap()
        .unwrap();

        assert_eq!(metadata["skill"], "keito-time-track");
        assert_eq!(metadata["duration_seconds"], 900);
    }

    #[test]
    fn agent_log_lifecycle_metadata_checks_final_size() {
        let raw = serde_json::json!({
            "session_id": "session-123",
            "padding": "x".repeat(4020),
        })
        .to_string();
        assert!(raw.len() <= 4096);

        let error = prepare_agent_log_metadata(
            build_metadata(MetadataInput {
                metadata: Some(raw),
                session_id: None,
                agent_id: None,
                agent_type: None,
                skill: None,
            })
            .unwrap(),
            "agent",
            900,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            AppError::InvalidInput(message)
                if message == "--metadata payload must be 4KB or smaller"
        ));
    }
}
