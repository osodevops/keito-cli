use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, RETRY_AFTER, USER_AGENT};
use reqwest::Client as HttpClient;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::error::map_status_to_error;
use crate::api::models::*;
use crate::config::ResolvedAuth;
use crate::error::AppError;

pub struct KeitorClient {
    client: HttpClient,
    base_url: String,
}

const MAX_ATTEMPTS: usize = 3;
static IDEMPOTENCY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("keito-cli/", env!("CARGO_PKG_VERSION"))),
        );
        headers.insert("X-Keito-Client", HeaderValue::from_static("cli"));
        headers.insert(
            "X-Keito-Client-Version",
            HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
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
        let idempotency_key = idempotency_key_for(&method);
        let mut last_error = None;
        let mut next_delay = None;

        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                let delay = next_delay
                    .take()
                    .unwrap_or_else(|| exponential_delay(attempt));
                tokio::time::sleep(delay).await;
            }

            let mut req = self.client.request(method.clone(), &url);
            if let Some(key) = idempotency_key.as_deref() {
                req = req.header("Idempotency-Key", key);
            }
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
            let retry_after = retry_after_seconds(resp.headers());

            if (200..300).contains(&status) {
                let text = resp
                    .text()
                    .await
                    .map_err(|e| AppError::Network(e.to_string()))?;
                let parsed: T = serde_json::from_str(&text).map_err(|e| {
                    AppError::ServerError(format!(
                        "Failed to parse successful {method} {path} response: {e}"
                    ))
                })?;
                return Ok(parsed);
            }

            let resp_body = resp.text().await.unwrap_or_default();

            // Mutations carry a stable idempotency key across attempts, so a
            // transient server failure is safe to retry just like a GET.
            if status >= 500 && attempt + 1 < MAX_ATTEMPTS {
                next_delay = retry_after.map(Duration::from_secs);
                last_error = Some(map_status_to_error(status, &resp_body, retry_after));
                continue;
            }

            // Client errors are not retried
            return Err(map_status_to_error(status, &resp_body, retry_after));
        }

        Err(last_error.unwrap_or_else(|| AppError::Network("Request failed after retries".into())))
    }

    async fn request_with_retry_no_body(
        &self,
        method: reqwest::Method,
        path: &str,
        extra_headers: Option<&HeaderMap>,
    ) -> Result<(), AppError> {
        let url = format!("{}{}", self.base_url, path);
        let idempotency_key = idempotency_key_for(&method);
        let mut last_error = None;
        let mut next_delay = None;

        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                let delay = next_delay
                    .take()
                    .unwrap_or_else(|| exponential_delay(attempt));
                tokio::time::sleep(delay).await;
            }

            let mut req = self.client.request(method.clone(), &url);
            if let Some(key) = idempotency_key.as_deref() {
                req = req.header("Idempotency-Key", key);
            }
            if let Some(headers) = extra_headers {
                req = req.headers(headers.clone());
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last_error = Some(AppError::Network(e.to_string()));
                    continue;
                }
            };

            let status = resp.status().as_u16();
            let retry_after = retry_after_seconds(resp.headers());

            if (200..300).contains(&status) {
                return Ok(());
            }

            let resp_body = resp.text().await.unwrap_or_default();

            if status >= 500 && attempt + 1 < MAX_ATTEMPTS {
                next_delay = retry_after.map(Duration::from_secs);
                last_error = Some(map_status_to_error(status, &resp_body, retry_after));
                continue;
            }

            return Err(map_status_to_error(status, &resp_body, retry_after));
        }

        Err(last_error.unwrap_or_else(|| AppError::Network("Request failed after retries".into())))
    }

    // ── API Methods ──

    pub async fn get_me(&self) -> Result<MeResponse, AppError> {
        self.request_with_retry::<MeResponse>(reqwest::Method::GET, "/api/v2/users/me", None::<&()>)
            .await
    }

    pub async fn list_clients(&self) -> Result<Vec<Client>, AppError> {
        let mut page = 1_u64;
        let mut clients = Vec::new();
        loop {
            let page_value = page.to_string();
            let path = path_with_query(
                "/api/v2/clients",
                &[
                    ("is_active", "true"),
                    ("per_page", "200"),
                    ("page", &page_value),
                ],
            );
            let resp: ClientsResponse = self
                .request_with_retry(reqwest::Method::GET, &path, None::<&()>)
                .await?;
            let has_next = response_has_next_page(resp.page, resp.total_pages, &resp.links);
            clients.extend(resp.clients);
            if !has_next {
                return Ok(clients);
            }
            page += 1;
        }
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
        let mut page = 1_u64;
        let mut projects = Vec::new();
        loop {
            let page_value = page.to_string();
            let mut query = vec![
                ("is_active", "true"),
                ("per_page", "200"),
                ("page", page_value.as_str()),
            ];
            if let Some(client_id) = client_id {
                query.push(("client_id", client_id));
            }
            let path = path_with_query("/api/v2/projects", &query);
            let resp: ProjectsResponse = self
                .request_with_retry(reqwest::Method::GET, &path, None::<&()>)
                .await?;
            let has_next = response_has_next_page(resp.page, resp.total_pages, &resp.links);
            projects.extend(resp.projects);
            if !has_next {
                return Ok(projects);
            }
            page += 1;
        }
    }

    pub async fn create_project(&self, req: &CreateProjectRequest) -> Result<Project, AppError> {
        self.request_with_retry(reqwest::Method::POST, "/api/v2/projects", Some(req))
            .await
    }

    pub async fn list_tasks(&self) -> Result<Vec<Task>, AppError> {
        self.list_tasks_for_project(None).await
    }

    pub async fn list_tasks_for_project(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<Task>, AppError> {
        let mut page = 1_u64;
        let mut tasks = Vec::new();
        loop {
            let page_value = page.to_string();
            let mut query = vec![
                ("is_active", "true"),
                ("per_page", "200"),
                ("page", page_value.as_str()),
            ];
            if let Some(project_id) = project_id {
                query.push(("project_id", project_id));
            }
            let path = path_with_query("/api/v2/tasks", &query);
            let resp: TasksResponse = self
                .request_with_retry(reqwest::Method::GET, &path, None::<&()>)
                .await?;
            let has_next = response_has_next_page(resp.page, resp.total_pages, &resp.links);
            tasks.extend(resp.tasks);
            if !has_next {
                return Ok(tasks);
            }
            page += 1;
        }
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

    pub async fn delete_time_entry(&self, id: &str) -> Result<(), AppError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Keito-Time-Entry-Delete-Intent",
            HeaderValue::from_static("discard-running"),
        );
        self.request_with_retry_no_body(
            reqwest::Method::DELETE,
            &format!("/api/v2/time_entries/{id}"),
            Some(&headers),
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

fn response_has_next_page(
    page: Option<u64>,
    total_pages: Option<u64>,
    links: &Option<PaginationLinks>,
) -> bool {
    if let Some(links) = links {
        return links.next.is_some();
    }
    matches!((page, total_pages), (Some(page), Some(total)) if page < total)
}

fn exponential_delay(retry_number: usize) -> Duration {
    Duration::from_secs(1_u64 << retry_number.saturating_sub(1).min(5))
}

fn retry_after_seconds(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn idempotency_key_for(method: &reqwest::Method) -> Option<String> {
    if !matches!(
        *method,
        reqwest::Method::POST | reqwest::Method::PATCH | reqwest::Method::DELETE
    ) {
        return None;
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = IDEMPOTENCY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Some(format!(
        "keito-cli-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}
