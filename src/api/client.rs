use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::Client as HttpClient;
use std::time::Duration;

use crate::api::error::map_status_to_error;
use crate::api::models::*;
use crate::config::ResolvedAuth;
use crate::error::AppError;

pub struct KeitorClient {
    client: HttpClient,
    base_url: String,
}

impl KeitorClient {
    pub fn new(auth: &ResolvedAuth, base_url: &str) -> Result<Self, AppError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", auth.api_key))
                .map_err(|_| AppError::Auth("Invalid API key format".into()))?,
        );
        headers.insert(
            "Keito-Account-Id",
            HeaderValue::from_str(&auth.workspace_id)
                .map_err(|_| AppError::Auth("Invalid workspace ID format".into()))?,
        );

        let client = HttpClient::builder()
            .default_headers(headers.clone())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| AppError::Network(format!("Failed to create HTTP client: {e}")))?;

        Ok(KeitorClient {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    async fn request_with_retry<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl serde::Serialize>,
    ) -> Result<T, AppError> {
        let url = format!("{}{}", self.base_url, path);
        let max_retries = 3;
        let mut last_error = None;

        for attempt in 0..max_retries {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << attempt);
                tokio::time::sleep(delay).await;
            }

            let mut req = self.client.request(method.clone(), &url);
            if let Some(b) = body {
                req = req.json(b);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last_error = Some(AppError::Network(e.to_string()));
                    continue;
                }
            };

            let status = resp.status().as_u16();

            if (200..300).contains(&status) {
                let text = resp
                    .text()
                    .await
                    .map_err(|e| AppError::Network(e.to_string()))?;
                let parsed: T = serde_json::from_str(&text)
                    .map_err(|e| AppError::ServerError(format!("Failed to parse response: {e}")))?;
                return Ok(parsed);
            }

            let resp_body = resp.text().await.unwrap_or_default();

            // Only retry on 5xx or network errors
            if status >= 500 {
                last_error = Some(map_status_to_error(status, &resp_body));
                continue;
            }

            // Client errors are not retried
            return Err(map_status_to_error(status, &resp_body));
        }

        Err(last_error.unwrap_or_else(|| AppError::Network("Request failed after retries".into())))
    }

    async fn request_with_retry_no_body(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<(), AppError> {
        let url = format!("{}{}", self.base_url, path);
        let max_retries = 3;
        let mut last_error = None;

        for attempt in 0..max_retries {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << attempt);
                tokio::time::sleep(delay).await;
            }

            let req = self.client.request(method.clone(), &url);

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last_error = Some(AppError::Network(e.to_string()));
                    continue;
                }
            };

            let status = resp.status().as_u16();

            if (200..300).contains(&status) {
                return Ok(());
            }

            let resp_body = resp.text().await.unwrap_or_default();

            if status >= 500 {
                last_error = Some(map_status_to_error(status, &resp_body));
                continue;
            }

            return Err(map_status_to_error(status, &resp_body));
        }

        Err(last_error.unwrap_or_else(|| AppError::Network("Request failed after retries".into())))
    }

    // ── API Methods ──

    pub async fn get_me(&self) -> Result<MeResponse, AppError> {
        self.request_with_retry::<MeResponse>(reqwest::Method::GET, "/api/v2/users/me", None::<&()>)
            .await
    }

    pub async fn list_clients(&self) -> Result<Vec<Client>, AppError> {
        let path = path_with_query(
            "/api/v2/clients",
            &[("is_active", "true"), ("per_page", "200")],
        );
        let resp: ClientsResponse = self
            .request_with_retry(reqwest::Method::GET, &path, None::<&()>)
            .await?;
        Ok(resp.clients)
    }

    pub async fn create_client(&self, req: &CreateClientRequest) -> Result<Client, AppError> {
        self.request_with_retry(reqwest::Method::POST, "/api/v2/clients", Some(req))
            .await
    }

    pub async fn list_projects(&self) -> Result<Vec<Project>, AppError> {
        self.list_projects_for_client(None).await
    }

    pub async fn list_projects_for_client(
        &self,
        client_id: Option<&str>,
    ) -> Result<Vec<Project>, AppError> {
        let path = if let Some(client_id) = client_id {
            path_with_query(
                "/api/v2/projects",
                &[
                    ("is_active", "true"),
                    ("per_page", "200"),
                    ("client_id", client_id),
                ],
            )
        } else {
            path_with_query(
                "/api/v2/projects",
                &[("is_active", "true"), ("per_page", "200")],
            )
        };
        let resp: ProjectsResponse = self
            .request_with_retry(reqwest::Method::GET, &path, None::<&()>)
            .await?;
        Ok(resp.projects)
    }

    pub async fn create_project(&self, req: &CreateProjectRequest) -> Result<Project, AppError> {
        self.request_with_retry(reqwest::Method::POST, "/api/v2/projects", Some(req))
            .await
    }

    pub async fn list_tasks(&self) -> Result<Vec<Task>, AppError> {
        let path = path_with_query(
            "/api/v2/tasks",
            &[("is_active", "true"), ("per_page", "200")],
        );
        let resp: TasksResponse = self
            .request_with_retry(reqwest::Method::GET, &path, None::<&()>)
            .await?;
        Ok(resp.tasks)
    }

    pub async fn list_time_entries(&self, params: &str) -> Result<Vec<TimeEntry>, AppError> {
        let path = if params.is_empty() {
            "/api/v2/time_entries?per_page=200".to_string()
        } else if params.contains("per_page=") {
            format!("/api/v2/time_entries?{params}")
        } else {
            format!("/api/v2/time_entries?{params}&per_page=200")
        };
        let resp: TimeEntriesResponse = self
            .request_with_retry(reqwest::Method::GET, &path, None::<&()>)
            .await?;
        Ok(resp.time_entries)
    }

    pub async fn create_time_entry(
        &self,
        req: &CreateTimeEntryRequest,
    ) -> Result<TimeEntry, AppError> {
        self.request_with_retry(reqwest::Method::POST, "/api/v2/time_entries", Some(req))
            .await
    }

    #[allow(dead_code)]
    pub async fn update_time_entry(
        &self,
        id: &str,
        req: &UpdateTimeEntryRequest,
    ) -> Result<TimeEntry, AppError> {
        self.request_with_retry(
            reqwest::Method::PATCH,
            &format!("/api/v2/time_entries/{id}"),
            Some(req),
        )
        .await
    }

    pub async fn stop_time_entry(
        &self,
        id: &str,
        notes: Option<&str>,
    ) -> Result<TimeEntry, AppError> {
        let path = format!("/api/v2/time_entries/{id}/stop");
        if let Some(notes) = notes {
            let req = StopTimeEntryRequest {
                notes: Some(notes.to_string()),
            };
            self.request_with_retry(reqwest::Method::PATCH, &path, Some(&req))
                .await
        } else {
            self.request_with_retry::<TimeEntry>(reqwest::Method::PATCH, &path, None::<&()>)
                .await
        }
    }

    pub async fn stop_time_entry_compat(
        &self,
        timer: &TimeEntry,
        notes: Option<&str>,
    ) -> Result<TimeEntry, AppError> {
        match self.stop_time_entry(&timer.id, notes).await {
            Ok(entry) => Ok(entry),
            Err(err) if is_missing_stop_route(&err) => {
                self.emulate_stop_time_entry(timer, notes).await
            }
            Err(err) => Err(err),
        }
    }

    async fn emulate_stop_time_entry(
        &self,
        timer: &TimeEntry,
        notes: Option<&str>,
    ) -> Result<TimeEntry, AppError> {
        let project_id = timer.project_id.clone().ok_or_else(|| {
            AppError::ServerError("Running timer response did not include project_id".into())
        })?;
        let task_id = timer.task_id.clone().ok_or_else(|| {
            AppError::ServerError("Running timer response did not include task_id".into())
        })?;

        let started_at = timer.timer_started_at.or(timer.created_at).ok_or_else(|| {
            AppError::ServerError("Running timer response did not include a start time".into())
        })?;
        let elapsed_seconds = (Utc::now() - started_at).num_seconds().max(60);
        let elapsed_hours =
            ((timer.hours.unwrap_or(0.0) + elapsed_seconds as f64 / 3600.0) * 100.0).round()
                / 100.0;

        let spent_date = timer
            .spent_date
            .unwrap_or_else(|| Utc::now().date_naive())
            .to_string();

        let stopped = self
            .create_time_entry(&CreateTimeEntryRequest {
                project_id,
                task_id,
                spent_date,
                hours: Some(elapsed_hours),
                notes: notes.map(str::to_string).or_else(|| {
                    timer
                        .notes
                        .as_ref()
                        .filter(|note| !note.is_empty())
                        .cloned()
                }),
                billable: Some(timer.billable),
                is_running: false,
                started_time: timer.started_time.clone(),
                ended_time: timer.ended_time.clone(),
                source: Some("cli".into()),
                metadata: timer.metadata.clone(),
            })
            .await?;

        self.delete_time_entry(&timer.id).await?;

        Ok(stopped)
    }

    pub async fn delete_time_entry(&self, id: &str) -> Result<(), AppError> {
        self.request_with_retry_no_body(
            reqwest::Method::DELETE,
            &format!("/api/v2/time_entries/{id}"),
        )
        .await
    }
}

fn path_with_query(path: &str, query: &[(&str, &str)]) -> String {
    let url = reqwest::Url::parse_with_params(&format!("https://keito.local{path}"), query)
        .expect("static API path should be a valid URL");

    match url.query() {
        Some(query) => format!("{}?{}", url.path(), query),
        None => url.path().to_string(),
    }
}

fn is_missing_stop_route(err: &AppError) -> bool {
    matches!(
        err,
        AppError::NotFound(message)
            if message.contains("This page could not be found")
                || message.contains("<!DOCTYPE html")
    )
}
