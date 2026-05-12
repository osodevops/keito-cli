use chrono::{Duration as ChronoDuration, Utc};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

use keito_cli::api::client::KeitorClient;
use keito_cli::config::ResolvedAuth;

fn test_auth(workspace_id: &str) -> ResolvedAuth {
    ResolvedAuth {
        api_key: "kto_test_key".into(),
        workspace_id: workspace_id.into(),
        api_key_source: "test".into(),
    }
}

fn fixture(name: &str) -> serde_json::Value {
    let contents = match name {
        "users_me" => include_str!("fixtures/api_v2/users_me.json"),
        "clients_list" => include_str!("fixtures/api_v2/clients_list.json"),
        "projects_list" => include_str!("fixtures/api_v2/projects_list.json"),
        "tasks_list" => include_str!("fixtures/api_v2/tasks_list.json"),
        "time_entries_list" => include_str!("fixtures/api_v2/time_entries_list.json"),
        "time_entry_create" => include_str!("fixtures/api_v2/time_entry_create.json"),
        "time_entry_running" => include_str!("fixtures/api_v2/time_entry_running.json"),
        "time_entry_stopped" => include_str!("fixtures/api_v2/time_entry_stopped.json"),
        "error_401" => include_str!("fixtures/api_v2/error_401.json"),
        "error_404" => include_str!("fixtures/api_v2/error_404.json"),
        "error_409" => include_str!("fixtures/api_v2/error_409.json"),
        other => panic!("unknown fixture {other}"),
    };

    serde_json::from_str(contents).unwrap()
}

struct EmptyBody;

impl Match for EmptyBody {
    fn matches(&self, request: &Request) -> bool {
        request.body.is_empty()
    }
}

#[tokio::test]
async fn get_me_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/users/me"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("users_me")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let me = client.get_me().await.unwrap();

    assert_eq!(me.display_name(), "Test User");
    assert_eq!(me.email, "test@test.com");
    assert_eq!(me.company.name, "Test Co");
}

#[tokio::test]
async fn list_clients_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/clients"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("clients_list")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let clients = client.list_clients().await.unwrap();

    assert_eq!(clients.len(), 2);
    assert_eq!(clients[0].name, "Client A");
    assert_eq!(clients[0].currency.as_deref(), Some("USD"));
    assert!(clients[0].is_active);
}

#[tokio::test]
async fn create_client_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v2/clients"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .and(body_json(serde_json::json!({
            "name": "Client A",
            "currency": "USD"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "c1",
            "name": "Client A",
            "currency": "USD",
            "address": null,
            "is_active": true
        })))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let created = client
        .create_client(&keito_cli::api::models::CreateClientRequest {
            name: "Client A".into(),
            address: None,
            currency: Some("USD".into()),
        })
        .await
        .unwrap();

    assert_eq!(created.id, "c1");
    assert_eq!(created.name, "Client A");
    assert_eq!(created.currency.as_deref(), Some("USD"));
}

#[tokio::test]
async fn get_me_unauthorized() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/users/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(fixture("error_401")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let result = client.get_me().await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.exit_code(), 1);
}

#[tokio::test]
async fn list_projects_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/projects"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("projects_list")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let projects = client.list_projects().await.unwrap();

    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].name, "Project A");
    assert_eq!(projects[0].code.as_deref(), Some("PA"));
    assert_eq!(projects[0].client_name(), Some("Client A"));
}

#[tokio::test]
async fn list_projects_can_filter_by_client() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/projects"))
        .and(query_param("client_id", "c1"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("projects_list")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let projects = client.list_projects_for_client(Some("c1")).await.unwrap();

    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].client_name(), Some("Client A"));
}

#[tokio::test]
async fn list_projects_encodes_client_filter() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/projects"))
        .and(query_param("client_id", "c 1/2"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("projects_list")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let projects = client
        .list_projects_for_client(Some("c 1/2"))
        .await
        .unwrap();

    assert_eq!(projects.len(), 2);
}

#[tokio::test]
async fn create_project_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v2/projects"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .and(body_json(serde_json::json!({
            "client_id": "c1",
            "name": "Agent Project",
            "code": "AP",
            "is_billable": true,
            "bill_by": "PROJECT",
            "budget_by": "NONE",
            "task_ids": ["t1"]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "p_agent",
            "client": { "id": "c1", "name": "Client A" },
            "name": "Agent Project",
            "code": "AP",
            "is_active": true,
            "is_billable": true,
            "is_fixed_fee": false,
            "bill_by": "PROJECT",
            "budget_by": "NONE"
        })))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let project = client
        .create_project(&keito_cli::api::models::CreateProjectRequest {
            client_id: "c1".into(),
            name: "Agent Project".into(),
            code: Some("AP".into()),
            notes: None,
            is_billable: Some(true),
            bill_by: Some("PROJECT".into()),
            budget_by: Some("NONE".into()),
            task_ids: Some(vec!["t1".into()]),
        })
        .await
        .unwrap();

    assert_eq!(project.id, "p_agent");
    assert_eq!(project.name, "Agent Project");
    assert_eq!(project.client_name(), Some("Client A"));
}

#[tokio::test]
async fn create_project_conflict() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v2/projects"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
            "error": "conflict",
            "error_description": "A project with this code already exists in your account"
        })))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let result = client
        .create_project(&keito_cli::api::models::CreateProjectRequest {
            client_id: "c1".into(),
            name: "Agent Project".into(),
            code: Some("AP".into()),
            notes: None,
            is_billable: Some(true),
            bill_by: Some("PROJECT".into()),
            budget_by: Some("NONE".into()),
            task_ids: None,
        })
        .await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), 3);
}

#[tokio::test]
async fn list_tasks_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/tasks"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("tasks_list")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let tasks = client.list_tasks().await.unwrap();

    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].name, "Development");
    assert!(tasks[0].billable_by_default);
}

#[tokio::test]
async fn list_time_entries_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/time_entries"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("time_entries_list")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let entries = client
        .list_time_entries("project_id=p1&per_page=10")
        .await
        .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].project_name(), Some("Project A"));
    assert_eq!(entries[0].task_name(), Some("Development"));
    assert_eq!(entries[0].spent_date.unwrap().to_string(), "2026-03-04");
    assert_eq!(entries[0].source.as_deref(), Some("cli"));
}

#[tokio::test]
async fn create_time_entry_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v2/time_entries"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .and(body_json(serde_json::json!({
            "project_id": "p1",
            "task_id": "t1",
            "spent_date": "2026-03-04",
            "hours": 1.5,
            "notes": "test",
            "billable": true,
            "is_running": false,
            "started_time": "09:00",
            "ended_time": "10:30",
            "source": "cli",
            "metadata": {"tool": "keito-cli"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("time_entry_create")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();

    let req = keito_cli::api::models::CreateTimeEntryRequest {
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
        metadata: Some(serde_json::json!({"tool": "keito-cli"})),
    };

    let entry = client.create_time_entry(&req).await.unwrap();
    assert_eq!(entry.id, "te_1");
    assert!(!entry.is_running);
    assert_eq!(entry.project_name(), Some("Project A"));
    assert!(entry.billable);
}

#[tokio::test]
async fn update_time_entry_success() {
    let server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/api/v2/time_entries/te_1"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .and(body_json(serde_json::json!({
            "is_running": false,
            "hours": 1.5
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("time_entry_create")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();

    let req = keito_cli::api::models::UpdateTimeEntryRequest {
        project_id: None,
        task_id: None,
        spent_date: None,
        is_running: Some(false),
        notes: None,
        hours: Some(1.5),
        billable: None,
        started_time: None,
        ended_time: None,
        metadata: None,
    };

    let entry = client.update_time_entry("te_1", &req).await.unwrap();
    assert_eq!(entry.hours, Some(1.5));
}

#[tokio::test]
async fn stop_time_entry_success_with_notes() {
    let server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/api/v2/time_entries/te_running/stop"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .and(body_json(serde_json::json!({"notes": "done"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("time_entry_stopped")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();

    let entry = client
        .stop_time_entry("te_running", Some("done"))
        .await
        .unwrap();

    assert_eq!(entry.id, "te_running");
    assert!(!entry.is_running);
    assert_eq!(entry.hours, Some(1.5));
    assert_eq!(entry.notes.as_deref(), Some("done"));
}

#[tokio::test]
async fn stop_time_entry_without_notes_sends_no_body() {
    let server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/api/v2/time_entries/te_running/stop"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .and(EmptyBody)
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("time_entry_stopped")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();

    let entry = client.stop_time_entry("te_running", None).await.unwrap();

    assert_eq!(entry.id, "te_running");
    assert!(!entry.is_running);
}

#[tokio::test]
async fn stop_time_entry_not_running_conflict() {
    let server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/api/v2/time_entries/te_running/stop"))
        .respond_with(ResponseTemplate::new(409).set_body_json(fixture("error_409")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();

    let result = client.stop_time_entry("te_running", None).await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), 3);
}

#[tokio::test]
async fn stop_time_entry_compat_falls_back_when_stop_route_is_missing() {
    let server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/api/v2/time_entries/te_running/stop"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string("<!DOCTYPE html>This page could not be found.</html>"),
        )
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v2/time_entries"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .and(body_json(serde_json::json!({
            "project_id": "p1",
            "task_id": "t1",
            "spent_date": "2026-03-04",
            "hours": 1.5,
            "notes": "done",
            "billable": true,
            "is_running": false,
            "started_time": "09:00",
            "source": "cli",
            "metadata": {"tool": "keito-cli"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("time_entry_create")))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("DELETE"))
        .and(path("/api/v2/time_entries/te_running"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let mut timer: keito_cli::api::models::TimeEntry =
        serde_json::from_value(fixture("time_entry_running")["time_entries"][0].clone()).unwrap();
    let started_at = Utc::now() - ChronoDuration::minutes(90);
    timer.timer_started_at = Some(started_at);
    timer.created_at = Some(started_at);

    let entry = client
        .stop_time_entry_compat(&timer, Some("done"))
        .await
        .unwrap();

    assert_eq!(entry.id, "te_1");
    assert!(!entry.is_running);
}

#[tokio::test]
async fn not_found_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/users/me"))
        .respond_with(ResponseTemplate::new(404).set_body_json(fixture("error_404")))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let result = client.get_me().await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), 4);
}

#[tokio::test]
async fn delete_time_entry_success() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v2/time_entries/te_1"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let result = client.delete_time_entry("te_1").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn rate_limited_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/users/me"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let auth = test_auth("co_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let result = client.get_me().await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), 5);
}
