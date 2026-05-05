use tabled::settings::Style;
use tabled::{Table, Tabled};

use crate::api::models::{MeResponse, Project, Task, TimeEntry};
use crate::types::format_duration;

#[allow(dead_code)]
pub trait TableDisplay {
    fn to_row(&self) -> Vec<TableRow>;
}

#[derive(Tabled)]
pub struct TableRow {
    pub key: String,
    pub value: String,
}

// ── Project table ──

#[derive(Tabled)]
pub struct ProjectRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "Name")]
    pub name: String,
    #[tabled(rename = "Code")]
    pub code: String,
    #[tabled(rename = "Client")]
    pub client: String,
    #[tabled(rename = "Billable")]
    pub billable: String,
    #[tabled(rename = "Active")]
    pub active: String,
}

impl TableDisplay for Project {
    fn to_row(&self) -> Vec<TableRow> {
        vec![]
    }
}

// ── Task table ──

#[derive(Tabled)]
pub struct TaskRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "Name")]
    pub name: String,
    #[tabled(rename = "Billable")]
    pub billable: String,
}

impl TableDisplay for Task {
    fn to_row(&self) -> Vec<TableRow> {
        vec![]
    }
}

// ── TimeEntry table ──

#[derive(Tabled)]
pub struct TimeEntryRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "Date")]
    pub date: String,
    #[tabled(rename = "Project")]
    pub project: String,
    #[tabled(rename = "Task")]
    pub task: String,
    #[tabled(rename = "Duration")]
    pub duration: String,
    #[tabled(rename = "Notes")]
    pub notes: String,
    #[tabled(rename = "Running")]
    pub running: String,
}

impl TableDisplay for TimeEntry {
    fn to_row(&self) -> Vec<TableRow> {
        vec![]
    }
}

impl TableDisplay for MeResponse {
    fn to_row(&self) -> Vec<TableRow> {
        vec![]
    }
}

// Single-item wrapper for serde references
impl<T: TableDisplay> TableDisplay for &T {
    fn to_row(&self) -> Vec<TableRow> {
        (*self).to_row()
    }
}

pub fn to_table<T>(items: &[T]) -> String
where
    T: serde::Serialize,
{
    // Use type-specific formatting via downcast-like approach
    // Since we can't easily downcast generics, we use serde to convert to a generic table
    to_table_from_serde(items)
}

fn to_table_from_serde<T: serde::Serialize>(items: &[T]) -> String {
    // Try to serialize as array of objects and extract columns
    let json = match serde_json::to_value(items) {
        Ok(v) => v,
        Err(_) => return "Error formatting table".into(),
    };

    let arr = match json.as_array() {
        Some(a) => a,
        None => return "Error formatting table".into(),
    };

    if arr.is_empty() {
        return "No results.".into();
    }

    // For known types, use typed table formatting
    if let Some(projects) = try_as_projects(arr) {
        return format_project_table(&projects);
    }
    if let Some(tasks) = try_as_tasks(arr) {
        return format_task_table(&tasks);
    }
    if let Some(entries) = try_as_time_entries(arr) {
        return format_time_entry_table(&entries);
    }
    if let Some(me) = try_as_me(arr) {
        return format_me_table(&me);
    }

    // Fallback: key-value pairs from first object
    if let Some(obj) = arr[0].as_object() {
        let rows: Vec<TableRow> = obj
            .iter()
            .map(|(k, v)| TableRow {
                key: k.clone(),
                value: match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
            })
            .collect();
        return Table::new(rows).with(Style::rounded()).to_string();
    }

    format!("{json}")
}

fn try_as_projects(arr: &[serde_json::Value]) -> Option<Vec<Project>> {
    // Check if items have project-specific fields
    let first = arr.first()?.as_object()?;
    if first.contains_key("is_active")
        && first.contains_key("name")
        && !first.contains_key("is_running")
    {
        serde_json::from_value(serde_json::Value::Array(arr.to_vec())).ok()
    } else {
        None
    }
}

fn try_as_tasks(arr: &[serde_json::Value]) -> Option<Vec<Task>> {
    let first = arr.first()?.as_object()?;
    let is_task = (first.contains_key("billable_by_default")
        && first.contains_key("name")
        && !first.contains_key("project_id")
        && !first.contains_key("is_active"))
        || (first.contains_key("is_active")
            && first.contains_key("billable_by_default")
            && !first.contains_key("code")
            && !first.contains_key("client"));
    if is_task {
        serde_json::from_value(serde_json::Value::Array(arr.to_vec())).ok()
    } else {
        None
    }
}

fn try_as_time_entries(arr: &[serde_json::Value]) -> Option<Vec<TimeEntry>> {
    let first = arr.first()?.as_object()?;
    if first.contains_key("is_running")
        || first.contains_key("timer_started_at")
        || first.contains_key("hours")
        || first.contains_key("spent_date")
    {
        serde_json::from_value(serde_json::Value::Array(arr.to_vec())).ok()
    } else {
        None
    }
}

fn try_as_me(arr: &[serde_json::Value]) -> Option<Vec<MeResponse>> {
    let first = arr.first()?.as_object()?;
    if first.contains_key("id") && first.contains_key("email") && first.contains_key("company") {
        serde_json::from_value(serde_json::Value::Array(arr.to_vec())).ok()
    } else {
        None
    }
}

fn format_project_table(projects: &[Project]) -> String {
    let rows: Vec<ProjectRow> = projects
        .iter()
        .map(|p| ProjectRow {
            id: p.id.clone(),
            name: p.name.clone(),
            code: p.code.clone().unwrap_or_default(),
            client: p.client_name().unwrap_or_default().to_string(),
            billable: if p.is_billable { "Yes" } else { "No" }.into(),
            active: if p.is_active { "Yes" } else { "No" }.into(),
        })
        .collect();
    Table::new(rows).with(Style::rounded()).to_string()
}

fn format_task_table(tasks: &[Task]) -> String {
    let rows: Vec<TaskRow> = tasks
        .iter()
        .map(|t| TaskRow {
            id: t.id.clone(),
            name: t.name.clone(),
            billable: if t.billable_by_default { "Yes" } else { "No" }.into(),
        })
        .collect();
    Table::new(rows).with(Style::rounded()).to_string()
}

fn format_time_entry_table(entries: &[TimeEntry]) -> String {
    let rows: Vec<TimeEntryRow> = entries
        .iter()
        .map(|e| TimeEntryRow {
            id: e.id.clone(),
            date: e.spent_date.map(|d| d.to_string()).unwrap_or_default(),
            project: e.project_name().unwrap_or_default().to_string(),
            task: e.task_name().unwrap_or_default().to_string(),
            duration: e.hours.map(format_duration).unwrap_or_else(|| {
                if e.is_running {
                    "running...".into()
                } else {
                    "-".into()
                }
            }),
            notes: e.notes.as_deref().unwrap_or("").chars().take(50).collect(),
            running: if e.is_running { "Yes" } else { "" }.into(),
        })
        .collect();
    Table::new(rows).with(Style::rounded()).to_string()
}

fn format_me_table(me_list: &[MeResponse]) -> String {
    if let Some(me) = me_list.first() {
        let rows = vec![
            TableRow {
                key: "User ID".into(),
                value: me.id.clone(),
            },
            TableRow {
                key: "Name".into(),
                value: me.display_name(),
            },
            TableRow {
                key: "Email".into(),
                value: me.email.clone(),
            },
            TableRow {
                key: "Company".into(),
                value: me.company.name.clone(),
            },
            TableRow {
                key: "Company ID".into(),
                value: me.company.id.clone(),
            },
        ];
        Table::new(rows).with(Style::rounded()).to_string()
    } else {
        "No data.".into()
    }
}
