use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

// -- Shared references --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedReference {
    pub id: String,
    pub name: String,
}

// -- User / Me --

pub type Company = NamedReference;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeResponse {
    pub id: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub user_type: Option<String>,
    pub company: Company,
}

impl MeResponse {
    pub fn display_name(&self) -> String {
        let first = self.first_name.as_deref().unwrap_or_default().trim();
        let last = self.last_name.as_deref().unwrap_or_default().trim();
        let full_name = format!("{first} {last}").trim().to_string();

        if !full_name.is_empty() {
            full_name
        } else {
            self.email.clone()
        }
    }
}

// -- Projects --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    #[serde(default)]
    pub client: Option<NamedReference>,
    pub name: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub is_billable: bool,
    #[serde(default)]
    pub is_fixed_fee: bool,
    #[serde(default)]
    pub bill_by: Option<String>,
    #[serde(default)]
    pub hourly_rate: Option<f64>,
    #[serde(default)]
    pub fee: Option<f64>,
    #[serde(default)]
    pub budget_by: Option<String>,
    #[serde(default)]
    pub budget: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl Project {
    pub fn client_name(&self) -> Option<&str> {
        self.client.as_ref().map(|client| client.name.as_str())
    }
}

// -- Tasks --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default, alias = "is_billable")]
    pub billable_by_default: bool,
    #[serde(default)]
    pub default_hourly_rate: Option<f64>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub parent_task_id: Option<String>,
}

// -- Time Entries --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeEntry {
    pub id: String,
    #[serde(default)]
    pub user: Option<NamedReference>,
    #[serde(default)]
    pub project: Option<NamedReference>,
    #[serde(default)]
    pub task: Option<NamedReference>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default, rename = "spent_date", alias = "date")]
    pub spent_date: Option<NaiveDate>,
    #[serde(default)]
    pub hours: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default, alias = "is_billable")]
    pub billable: bool,
    #[serde(default)]
    pub is_running: bool,
    #[serde(default)]
    pub timer_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub started_time: Option<String>,
    #[serde(default)]
    pub ended_time: Option<String>,
    #[serde(default)]
    pub is_locked: bool,
    #[serde(default)]
    pub is_closed: bool,
    #[serde(default)]
    pub is_billed: bool,
    #[serde(default)]
    pub budgeted: bool,
    #[serde(default)]
    pub billable_rate: Option<f64>,
    #[serde(default)]
    pub cost_rate: Option<f64>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

impl TimeEntry {
    pub fn project_name(&self) -> Option<&str> {
        self.project.as_ref().map(|project| project.name.as_str())
    }

    pub fn task_name(&self) -> Option<&str> {
        self.task.as_ref().map(|task| task.name.as_str())
    }
}

// -- Request bodies --

#[derive(Debug, Serialize)]
pub struct CreateTimeEntryRequest {
    pub project_id: String,
    pub task_id: String,
    pub spent_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hours: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billable: Option<bool>,
    pub is_running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct UpdateTimeEntryRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hours: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct StopTimeEntryRequest {
    pub notes: Option<String>,
}

// -- Pagination envelopes --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationLinks {
    pub first: String,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
    pub last: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectsResponse {
    pub projects: Vec<Project>,
    #[serde(default)]
    pub per_page: Option<u64>,
    #[serde(default)]
    pub total_pages: Option<u64>,
    #[serde(default)]
    pub total_entries: Option<u64>,
    #[serde(default)]
    pub page: Option<u64>,
    #[serde(default)]
    pub links: Option<PaginationLinks>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksResponse {
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub per_page: Option<u64>,
    #[serde(default)]
    pub total_pages: Option<u64>,
    #[serde(default)]
    pub total_entries: Option<u64>,
    #[serde(default)]
    pub page: Option<u64>,
    #[serde(default)]
    pub links: Option<PaginationLinks>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeEntriesResponse {
    pub time_entries: Vec<TimeEntry>,
    #[serde(default)]
    pub per_page: Option<u64>,
    #[serde(default)]
    pub total_pages: Option<u64>,
    #[serde(default)]
    pub total_entries: Option<u64>,
    #[serde(default)]
    pub page: Option<u64>,
    #[serde(default)]
    pub links: Option<PaginationLinks>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_entry_reads_production_nested_names() {
        let entry: TimeEntry = serde_json::from_value(serde_json::json!({
            "id": "te_1",
            "project": {"id": "p1", "name": "Project A"},
            "task": {"id": "t1", "name": "Development"},
            "project_id": "p1",
            "task_id": "t1",
            "spent_date": "2026-03-04",
            "hours": 1.5,
            "billable": true
        }))
        .unwrap();

        assert_eq!(entry.project_name(), Some("Project A"));
        assert_eq!(entry.task_name(), Some("Development"));
        assert_eq!(entry.spent_date.unwrap().to_string(), "2026-03-04");
        assert!(entry.billable);
    }

    #[test]
    fn create_time_entry_serializes_production_fields() {
        let req = CreateTimeEntryRequest {
            project_id: "p1".into(),
            task_id: "t1".into(),
            spent_date: "2026-03-04".into(),
            hours: Some(1.5),
            notes: Some("test".into()),
            billable: Some(true),
            is_running: false,
            source: Some("cli".into()),
            metadata: None,
        };

        let value = serde_json::to_value(req).unwrap();
        assert_eq!(value["spent_date"], "2026-03-04");
        assert_eq!(value["billable"], true);
        assert!(value.get("date").is_none());
        assert!(value.get("is_billable").is_none());
    }
}
