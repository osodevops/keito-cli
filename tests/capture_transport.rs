#![allow(deprecated)]
use assert_cmd::Command;
use keito_cli::capture::{self, Binding, Clock, Repository, Signal, Store};
use keito_cli::cli::capture::CaptureEvent::{Prompt, Stop};
use serde_json::{json, Value};
use std::path::Path;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
fn init(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
}
fn command(home: &Path, root: &Path, origin: &str) -> Command {
    let mut cmd = Command::cargo_bin("keito").unwrap();
    cmd.current_dir(root)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("AppData/Roaming"))
        .env("KEITO_CAPTURE_HOME", home.join("capture"))
        .env("KEITO_API_URL", origin)
        .env("KEITO_API_KEY", "kto_test_key")
        .env("KEITO_ACCOUNT_ID", "co_test");
    cmd
}
async fn me(server: &MockServer, owner: &str) {
    let mut value: Value =
        serde_json::from_str(include_str!("fixtures/api_v2/users_me.json")).unwrap();
    value["id"] = json!(owner);
    Mock::given(method("GET"))
        .and(path("/api/v2/users/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .mount(server)
        .await;
}
fn seed(home: &Path, root: &Path, origin: &str) -> (Store, Binding) {
    let repo = Repository::resolve(root).unwrap();
    let mut store = Store::open(&home.join("capture")).unwrap();
    let b = store
        .approve(Binding {
            repository_key: repo.key.clone(),
            hint_hash: repo.hint_hash.clone(),
            id: String::new(),
            version: 0,
            workspace_id: "co_test".into(),
            owner_id: "usr_1".into(),
            origin_hash: capture::hash(origin),
            project_id: "p1".into(),
            task_id: "t1".into(),
            agents: vec!["codex".into()],
            paused: false,
            revoked: false,
        })
        .unwrap();
    let clock = Clock {
        wall: 1_794_067_200,
        monotonic: 10,
        boot: "fixture".into(),
    };
    let end = Clock {
        wall: clock.wall + 90,
        monotonic: 100,
        boot: "fixture".into(),
    };
    for (event, time) in [(Prompt, &clock), (Stop, &end)] {
        store
            .event(
                &repo,
                Signal {
                    source: "codex",
                    resumed: false,
                    source_id: "synthetic-session",
                    turn_id: Some("synthetic-turn"),
                    version: "0.167.0",
                    event,
                    clock: time,
                },
            )
            .unwrap();
    }
    (store, b)
}
#[tokio::test]
async fn sync_sends_only_canonical_exact_metadata_and_acknowledges_once() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("repo");
    init(&root);
    me(&server, "usr_1").await;
    let (store, _) = seed(home.path(), &root, &server.uri());
    Mock::given(method("POST")).and(path("/api/v2/agent-sessions")).and(header("Authorization","Bearer kto_test_key")).and(header("Keito-Account-Id","co_test"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"agent_session":{"id":"canonical-session","revision":1,"projection_state":"PROJECTED","review_state":"PENDING"}}))).expect(1).mount(&server).await;
    command(home.path(), &root, &server.uri())
        .args(["capture", "sync", "--json"])
        .assert()
        .success();
    command(home.path(), &root, &server.uri())
        .args(["capture", "sync", "--json"])
        .assert()
        .success();
    assert_eq!(store.status().unwrap().pending, 0);
    assert!(store.status().unwrap().delivery_verified);
    let requests = server.received_requests().await.unwrap();
    let request = requests.iter().find(|r| r.method == "POST").unwrap();
    let payload: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(payload["segments"][0]["duration_seconds"], 90);
    assert_eq!(payload["usage"]["status"], "UNAVAILABLE");
    assert!(payload.get("metadata").is_none());
    assert!(!String::from_utf8_lossy(&request.body).contains(home.path().to_str().unwrap()));
    assert_eq!(
        request.headers["idempotency-key"].to_str().unwrap(),
        capture::hash(&request.body)
    );
}
#[tokio::test]
async fn revoked_owner_or_changed_mapping_never_uploads_pending_evidence() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("repo");
    init(&root);
    me(&server, "different_owner").await;
    let (store, _) = seed(home.path(), &root, &server.uri());
    command(home.path(), &root, &server.uri())
        .args(["capture", "sync", "--json"])
        .assert()
        .success();
    assert_eq!(store.status().unwrap().pending, 1);
    assert!(!server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .any(|r| r.method == "POST"));
}
#[tokio::test]
async fn transient_response_is_held_with_frozen_body_and_retry_after() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("repo");
    init(&root);
    me(&server, "usr_1").await;
    let (mut store, b) = seed(home.path(), &root, &server.uri());
    Mock::given(method("POST"))
        .and(path("/api/v2/agent-sessions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "60")
                .set_body_string("PRIVATE_SERVER_CANARY"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let out = command(home.path(), &root, &server.uri())
        .args(["capture", "sync", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE"));
    assert!(store
        .claim(&b, chrono::Utc::now().timestamp() + 30)
        .unwrap()
        .is_none());
    let queued = store
        .claim(&b, chrono::Utc::now().timestamp() + 65)
        .unwrap()
        .unwrap();
    let payload: Value = serde_json::from_str(&queued.body).unwrap();
    assert_eq!(payload["segments"][0]["duration_seconds"], 90);
}
#[tokio::test]
async fn setup_validates_auth_project_and_task_before_persisting_approval() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("repo");
    init(&root);
    me(&server, "usr_1").await;
    for (route, fixture) in [
        (
            "/api/v2/projects",
            include_str!("fixtures/api_v2/projects_list.json"),
        ),
        (
            "/api/v2/tasks",
            include_str!("fixtures/api_v2/tasks_list.json"),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_raw(fixture, "application/json"))
            .mount(&server)
            .await;
    }
    command(home.path(), &root, &server.uri())
        .args([
            "capture",
            "setup",
            "--agent",
            "codex",
            "--project",
            "p1",
            "--task",
            "wrong_task",
            "--confirm",
            "--json",
        ])
        .assert()
        .failure();
    assert!(!home.path().join("capture/capture.sqlite3").exists());
    command(home.path(), &root, &server.uri())
        .args([
            "capture",
            "setup",
            "--agent",
            "codex",
            "--project",
            "p1",
            "--task",
            "t1",
            "--confirm",
            "--json",
        ])
        .assert()
        .success();
    let store = Store::open(&home.path().join("capture")).unwrap();
    assert_eq!(store.status().unwrap().bindings.len(), 1);
}

#[tokio::test]
async fn paused_or_revoked_local_consent_performs_no_network_even_preflight() {
    for action in ["pause", "disable"] {
        let server = MockServer::start().await;
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("repo");
        init(&root);
        let (mut store, _) = seed(home.path(), &root, &server.uri());
        let repo = Repository::resolve(&root).unwrap();
        store
            .control(
                &repo,
                action,
                &Clock {
                    wall: 1_794_067_300,
                    monotonic: 110,
                    boot: "fixture".into(),
                },
            )
            .unwrap();
        command(home.path(), &root, &server.uri())
            .args(["capture", "sync", "--json"])
            .assert()
            .success();
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "Local disable must not cause authentication or capture requests"
        );
    }
}
