use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

// -- Shared references --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedReference {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

// -- User / Me --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Company {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_rounding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_past_timer_starts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_future_expense_dates: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal_time_notes_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal_time_notes_visibility: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_entry_notes_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_billability_overrides_enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_track_time: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_track_expenses: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_edit_time_billability: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_create_field_clients: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_view_team_time: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_view_invoices: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_send_invoices: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_record_invoice_payments: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_read_own_time_entries: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_read_users: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_read_own_profile: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_read_clients: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_read_projects: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeResponse {
    pub id: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telephone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_access_to_all_future_projects: Option<bool>,
    #[serde(default)]
    pub is_contractor: bool,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_capacity: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_hourly_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<Company>,
    #[serde(default)]
    pub capabilities: MeCapabilities,
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

    pub fn is_personal_read_only_sync(&self) -> bool {
        self.company.is_none()
            && self.capabilities.can_read_own_time_entries == Some(true)
            && self.capabilities.can_read_own_profile == Some(true)
    }

    pub fn credential_type(&self) -> &'static str {
        if self.is_personal_read_only_sync() {
            "personal_read_only_sync"
        } else if self.company.is_some() {
            "full_access"
        } else {
            "unknown"
        }
    }
}

// -- Clients --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_terms: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tax: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tax2: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discount: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement_key: Option<String>,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct CreateClientRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

// -- Projects --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    #[serde(default)]
    pub client: Option<NamedReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OwnerReference>,
    pub name: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_code: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_is_monthly: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_counts_billable_hours_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_when_over_budget: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over_budget_notification_percentage: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_budget_to_all: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget_include_expenses: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_on: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_on: Option<NaiveDate>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<Task>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerReference {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub email: String,
}

impl Project {
    pub fn client_name(&self) -> Option<&str> {
        self.client.as_ref()?.name.as_deref()
    }
}

#[derive(Debug, Serialize)]
pub struct CreateProjectRequest {
    pub client_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_billable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bill_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_billable_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<f64>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_assignment: Option<SyncTaskAssignment>,
    #[serde(default, rename = "spent_date", alias = "date")]
    pub spent_date: Option<NaiveDate>,
    #[serde(default)]
    pub hours: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounded_hours: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal_notes: Option<String>,
    #[serde(default, alias = "is_billable")]
    pub billable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_edit_billable: Option<bool>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_reference: Option<ExternalReference>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTaskAssignment {
    pub id: String,
    pub billable: bool,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalReference {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permalink: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

impl TimeEntry {
    pub fn project_name(&self) -> Option<&str> {
        self.project.as_ref()?.name.as_deref()
    }

    pub fn task_name(&self) -> Option<&str> {
        self.task.as_ref()?.name.as_deref()
    }

    pub fn actual_hours(&self) -> Option<f64> {
        self.duration_seconds
            .map(|seconds| seconds as f64 / 3600.0)
            .or(self.hours)
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
    pub started_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct UpdateTimeEntryRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spent_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hours: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_time: Option<String>,
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
pub struct ClientsResponse {
    pub clients: Vec<Client>,
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
            started_time: Some("09:00".into()),
            ended_time: Some("10:30".into()),
            source: Some("cli".into()),
            metadata: None,
        };

        let value = serde_json::to_value(req).unwrap();
        assert_eq!(value["spent_date"], "2026-03-04");
        assert_eq!(value["billable"], true);
        assert_eq!(value["started_time"], "09:00");
        assert_eq!(value["ended_time"], "10:30");
        assert!(value.get("date").is_none());
        assert!(value.get("is_billable").is_none());
    }
}
