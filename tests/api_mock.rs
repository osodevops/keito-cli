use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use keito_cli::api::client::KeitorClient;
use keito_cli::config::ResolvedAuth;

fn test_auth(workspace_id: &str) -> ResolvedAuth {
    ResolvedAuth {
        api_key: "kto_test_key".into(),
        workspace_id: workspace_id.into(),
        api_key_source: "test".into(),
    }
}

#[tokio::test]
async fn get_me_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/me"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "ws_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user": {"id": "usr_1", "name": "Test User", "email": "test@test.com"},
            "company": {"id": "ws_test", "name": "Test Co"}
        })))
        .mount(&server)
        .await;

    let auth = test_auth("ws_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let me = client.get_me().await.unwrap();

    assert_eq!(me.user.name, "Test User");
    assert_eq!(me.company.name, "Test Co");
}

#[tokio::test]
async fn get_me_unauthorized() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "unauthorized",
            "error_description": "Invalid API key"
        })))
        .mount(&server)
        .await;

    let auth = test_auth("ws_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let result = client.get_me().await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.exit_code(), 1); // Auth error
}

#[tokio::test]
async fn list_projects_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "p1", "name": "Project A", "code": "PA", "is_active": true, "is_billable": true},
                {"id": "p2", "name": "Project B", "is_active": true, "is_billable": false}
            ],
            "total": 2
        })))
        .mount(&server)
        .await;

    let auth = test_auth("ws_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let projects = client.list_projects().await.unwrap();

    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].name, "Project A");
    assert_eq!(projects[0].code.as_deref(), Some("PA"));
}

#[tokio::test]
async fn list_tasks_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/tasks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "t1", "name": "Development", "is_active": true, "is_billable": true},
                {"id": "t2", "name": "Design", "is_active": true, "is_billable": true}
            ]
        })))
        .mount(&server)
        .await;

    let auth = test_auth("ws_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let tasks = client.list_tasks().await.unwrap();

    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].name, "Development");
}

#[tokio::test]
async fn create_time_entry_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v2/time_entries"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": "te_1",
            "project_id": "p1",
            "project_name": "Project A",
            "task_id": "t1",
            "task_name": "Development",
            "is_running": true,
            "timer_started_at": "2026-03-04T10:00:00Z",
            "date": "2026-03-04"
        })))
        .mount(&server)
        .await;

    let auth = test_auth("ws_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();

    let req = keito_cli::api::models::CreateTimeEntryRequest {
        project_id: "p1".into(),
        task_id: "t1".into(),
        date: Some("2026-03-04".into()),
        hours: None,
        notes: Some("test".into()),
        is_billable: None,
        is_running: true,
    };

    let entry = client.create_time_entry(&req).await.unwrap();
    assert_eq!(entry.id, "te_1");
    assert!(entry.is_running);
}

#[tokio::test]
async fn update_time_entry_success() {
    let server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/api/v2/time_entries/te_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "te_1",
            "project_id": "p1",
            "project_name": "Project A",
            "task_id": "t1",
            "task_name": "Development",
            "is_running": false,
            "hours": 1.5,
            "date": "2026-03-04"
        })))
        .mount(&server)
        .await;

    let auth = test_auth("ws_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();

    let req = keito_cli::api::models::UpdateTimeEntryRequest {
        is_running: Some(false),
        notes: None,
        hours: Some(1.5),
    };

    let entry = client.update_time_entry("te_1", &req).await.unwrap();
    assert!(!entry.is_running);
    assert_eq!(entry.hours, Some(1.5));
}

#[tokio::test]
async fn not_found_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/me"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "error": "not_found",
            "message": "Resource not found"
        })))
        .mount(&server)
        .await;

    let auth = test_auth("ws_test");
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

    let auth = test_auth("ws_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let result = client.delete_time_entry("te_1").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn rate_limited_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/me"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let auth = test_auth("ws_test");
    let client = KeitorClient::new(&auth, &server.uri()).unwrap();
    let result = client.get_me().await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), 5);
}
