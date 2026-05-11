use chrono::{DateTime, Local, Utc};
use colored::Colorize;
use serde_json::{Map, Value};

use crate::api::models::{CreateTimeEntryRequest, TimeEntry, UpdateTimeEntryRequest};
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

    let entry = client
        .stop_time_entry_compat(&timer, notes.as_deref())
        .await?;

    if mode == OutputMode::Json {
        let out = serde_json::json!({
            "status": "stopped",
            "entry_id": entry.id,
            "project": entry.project_name(),
            "task": entry.task_name(),
            "duration_hours": entry.hours,
            "duration": entry.hours.map(format_duration),
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
            entry.hours.map(format_duration).unwrap_or_default(),
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
    let source = normalize_source(&source)?;
    let metadata = build_metadata(MetadataInput {
        metadata,
        session_id,
        agent_id,
        agent_type,
        skill,
    })?;

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
            "duration_hours": entry.hours,
            "duration": entry.hours.map(format_duration),
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
            entry.hours.map(format_duration).unwrap_or_default(),
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
    let metadata = build_metadata(MetadataInput {
        metadata,
        session_id: Some(session_id.clone()),
        agent_id,
        agent_type,
        skill,
    })?
    .unwrap_or_else(|| serde_json::json!({ "session_id": session_id.clone() }));

    let auth = ResolvedAuth::resolve(global)?;
    let config = AppConfig::load()?;
    let client = KeitorClient::new(&auth, &config.api_base_url())?;

    let projects = client.list_projects().await?;
    let project_items: Vec<(String, String, Option<String>)> = projects
        .iter()
        .map(|p| (p.id.clone(), p.name.clone(), p.code.clone()))
        .collect();
    let project_id = resolve_name_to_id(project_query, &project_items, "Project")?.to_string();

    let tasks = client.list_tasks().await?;
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
                    is_running: Some(false),
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
            "duration_hours": entry.hours,
            "duration": entry.hours.map(format_duration),
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
            entry.hours.map(format_duration).unwrap_or_default(),
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
                "project": entry.project_name(),
                "task": entry.task_name(),
                "started_at": entry.timer_started_at,
                "spent_date": entry.spent_date,
                "billable": entry.billable,
                "source": entry.source,
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
        "web" | "cli" | "api" | "agent" => Ok(normalized),
        _ => Err(AppError::InvalidInput(format!(
            "source must be one of: web, cli, api, agent (got '{source}')"
        ))),
    }
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
        assert!(normalize_source("desktop").is_err());
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
}
