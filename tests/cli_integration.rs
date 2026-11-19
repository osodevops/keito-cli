#![allow(deprecated)]
use assert_cmd::Command;
#[cfg(unix)]
#[path = "support/mutation_environment.rs"]
mod mutation_isolation;
use chrono::Local;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_test_config(home: &Path, api_url: &str) {
    let config = format!("api_url = \"{api_url}\"\n");

    let xdg_path = home.join("config").join("keito");
    fs::create_dir_all(&xdg_path).unwrap();
    fs::write(xdg_path.join("config.toml"), &config).unwrap();

    let mac_path = home
        .join("Library")
        .join("Application Support")
        .join("keito");
    fs::create_dir_all(&mac_path).unwrap();
    fs::write(mac_path.join("config.toml"), &config).unwrap();

    let windows_path = home.join("AppData").join("Roaming").join("keito");
    fs::create_dir_all(&windows_path).unwrap();
    fs::write(windows_path.join("config.toml"), config).unwrap();
}

fn command_with_mock_config(home: &Path, api_url: &str) -> Command {
    let mut cmd = Command::cargo_bin("keito").unwrap();
    cmd.env("HOME", home)
        .env("KEITO_CAPTURE_HOME", home.join("capture"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("AppData").join("Roaming"))
        .env("KEITO_API_URL", api_url)
        .env("KEITO_API_KEY", "kto_test_key")
        .env("KEITO_ACCOUNT_ID", "co_test")
        .env_remove("KEITO_WORKSPACE_ID");
    cmd
}

#[cfg(unix)]
fn write_fake_skill_tools(home: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = home.join("bin");
    fs::create_dir(&bin).unwrap();
    let npx = bin.join("npx");
    fs::write(
        &npx,
        "#!/bin/sh\nprintf forbidden > \"$KEITO_FAKE_NPX_LOG\"\nexit 89\n",
    )
    .unwrap();
    fs::set_permissions(&npx, fs::Permissions::from_mode(0o700)).unwrap();
    bin
}

#[test]
fn help_flag_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Track billable time"));
}

#[test]
fn auth_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["auth", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("login"));
}

#[test]
fn time_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["time", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("start"));
}

#[test]
fn projects_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["projects", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"));
}

#[test]
fn projects_create_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["projects", "create", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--client"))
        .stdout(predicate::str::contains("--task"));
}

#[tokio::test]
async fn projects_create_sends_normalized_body_against_mock_api() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());

    Mock::given(method("POST"))
        .and(path("/api/v2/projects"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .and(body_json(serde_json::json!({
            "client_id": "c1",
            "name": "Agent Project",
            "code": "AP",
            "is_billable": false,
            "bill_by": "NONE",
            "budget_by": "NONE",
            "task_ids": ["t1"]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "p_agent",
            "client": { "id": "c1", "name": "Client A" },
            "name": "Agent Project",
            "code": "AP",
            "is_active": true,
            "is_billable": false,
            "is_fixed_fee": false,
            "bill_by": "NONE",
            "budget_by": "NONE"
        })))
        .expect(1)
        .mount(&server)
        .await;

    command_with_mock_config(temp_dir.path(), &server.uri())
        .args([
            "--json",
            "projects",
            "create",
            " Agent Project ",
            "--client",
            " c1 ",
            "--code",
            " AP ",
            "--billable",
            "false",
            "--task",
            " t1 ",
            "--task",
            "t1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""id": "p_agent""#))
        .stdout(predicate::str::contains(r#""is_billable": false"#));
}

#[test]
fn clients_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["clients", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"));
}

#[test]
fn skill_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["skill", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("install"));
}

#[test]
#[cfg(unix)]
fn unqualified_source_build_refuses_before_external_installer_or_profile_mutation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let case = temp_dir.path().canonicalize().unwrap();
    let home = case.join("home");
    fs::create_dir(&home).unwrap();
    let capture = home.join("capture");
    fs::create_dir(&capture).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&capture, fs::Permissions::from_mode(0o700)).unwrap();
    let core = case.join("keito");
    fs::copy(assert_cmd::cargo::cargo_bin!("keito"), &core).unwrap();
    let environment =
        mutation_isolation::checked(&case, &case, Some(&home), Some(&capture), Some(&core))
            .unwrap();
    let bin = write_fake_skill_tools(&home);
    let log = home.join("npx.log");
    let mut cmd = Command::new(&environment.core);
    cmd.env_clear().env("PATH", &bin);
    cmd.env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("AppData"))
        .env("HOME", &environment.home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("KEITO_CAPTURE_HOME", &environment.capture)
        .env("KEITO_FAKE_NPX_LOG", &log)
        .args([
            "skill",
            "install",
            "--source",
            "/tmp/inert-unread-source",
            "--agent",
            "codex",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "official keito / keito-capture-guard release pair",
        ));
    assert!(!log.exists());
    assert!(!home.join(".codex").exists());
    assert!(!home.join(".claude").exists());
}

#[test]
#[cfg(unix)]
fn unqualified_source_build_refuses_bundled_install_without_touching_profiles() {
    let temp_dir = tempfile::tempdir().unwrap();
    let case = temp_dir.path().canonicalize().unwrap();
    let home = case.join("home");
    fs::create_dir(&home).unwrap();
    let capture = home.join("capture");
    fs::create_dir(&capture).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&capture, fs::Permissions::from_mode(0o700)).unwrap();
    let core = case.join("keito");
    fs::copy(assert_cmd::cargo::cargo_bin!("keito"), &core).unwrap();
    let environment =
        mutation_isolation::checked(&case, &case, Some(&home), Some(&capture), Some(&core))
            .unwrap();
    let bin = write_fake_skill_tools(&home);
    Command::new(&environment.core)
        .env_clear()
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("AppData"))
        .env("PATH", &bin)
        .env("HOME", &environment.home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("KEITO_CAPTURE_HOME", &environment.capture)
        .args([
            "skill",
            "install",
            "--agent",
            "codex",
            "--agent",
            "claude-code",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "official keito / keito-capture-guard release pair",
        ));
    assert!(!home.join(".codex").exists());
    assert!(!home.join(".claude").exists());
}

#[tokio::test]
async fn clients_list_sends_account_header_against_mock_api() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v2/clients"))
        .and(query_param("is_active", "true"))
        .and(query_param("per_page", "200"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "clients": [{
                "id": "c1",
                "name": "Client A",
                "currency": "USD",
                "address": null,
                "is_active": true,
                "created_at": "2026-05-12T08:00:00Z",
                "updated_at": "2026-05-12T08:00:00Z"
            }],
            "per_page": 200,
            "total_pages": 1,
            "total_entries": 1,
            "page": 1,
            "links": {
                "first": "/api/v2/clients?page=1&per_page=200",
                "next": null,
                "previous": null,
                "last": "/api/v2/clients?page=1&per_page=200"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    command_with_mock_config(temp_dir.path(), &server.uri())
        .args(["--json", "clients", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""id": "c1""#));
}

#[test]
fn whoami_without_auth_fails() {
    let temp_dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("keito")
        .unwrap()
        .args(["auth", "whoami"])
        .env("HOME", temp_dir.path())
        .env("KEITO_CAPTURE_HOME", temp_dir.path().join("capture"))
        .env("XDG_CONFIG_HOME", temp_dir.path().join("config"))
        .env_remove("KEITO_API_KEY")
        .env_remove("KEITO_ACCOUNT_ID")
        .env_remove("KEITO_API_URL")
        .env_remove("KEITO_WORKSPACE_ID")
        .assert()
        .failure()
        .code(1);
}

#[test]
fn time_stop_help_shows_discard() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["time", "stop", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--discard"));
}

#[test]
fn time_log_help_shows_agent_fields() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["time", "log", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--duration-seconds"))
        .stdout(predicate::str::contains("--metadata"))
        .stdout(predicate::str::contains("--source"));
}

#[test]
fn time_list_help_shows_today_and_source_filters() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["time", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--today"))
        .stdout(predicate::str::contains("--source"));
}

#[test]
fn time_session_record_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["time", "session-record", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--session-id"))
        .stdout(predicate::str::contains("--duration-seconds"));
}

#[tokio::test]
async fn time_session_record_creates_agent_entry_against_mock_api() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v2/projects"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "projects": [{
                "id": "p1",
                "client": { "id": "c1", "name": "Client A" },
                "name": "Project A",
                "code": "PA",
                "is_active": true,
                "is_billable": true
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v2/tasks"))
        .and(query_param("project_id", "p1"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tasks": [{
                "id": "t1",
                "name": "Development",
                "billable_by_default": true,
                "is_active": true
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v2/time_entries"))
        .and(header("Authorization", "Bearer kto_test_key"))
        .and(header("Keito-Account-Id", "co_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "time_entries": [],
            "per_page": 200,
            "total_pages": 1,
            "total_entries": 0,
            "page": 1
        })))
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
            "spent_date": "2026-05-11",
            "hours": 1.5,
            "notes": "agent work",
            "billable": true,
            "is_running": false,
            "source": "agent",
            "metadata": {
                "session_id": "sess_123",
                "agent_id": "codex",
                "agent_type": "codex",
                "skill": "keito-time-track",
                "duration_seconds": 5400
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "te_agent",
            "project": { "id": "p1", "name": "Project A" },
            "task": { "id": "t1", "name": "Development" },
            "project_id": "p1",
            "task_id": "t1",
            "spent_date": "2026-05-11",
            "hours": 1.5,
            "notes": "agent work",
            "billable": true,
            "is_running": false,
            "source": "agent",
            "metadata": { "session_id": "sess_123" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    command_with_mock_config(temp_dir.path(), &server.uri())
        .args([
            "--json",
            "time",
            "session-record",
            "--project",
            "PA",
            "--task",
            "Development",
            "--session-id",
            "sess_123",
            "--duration-seconds",
            "5400",
            "--date",
            "2026-05-11",
            "--notes",
            "agent work",
            "--billable",
            "true",
            "--agent-id",
            "codex",
            "--agent-type",
            "codex",
            "--skill",
            "keito-time-track",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status": "created""#))
        .stdout(predicate::str::contains(r#""session_id": "sess_123""#));
}

#[tokio::test]
async fn time_start_uses_tasks_embedded_for_selected_project() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());
    let spent_date = Local::now().format("%Y-%m-%d").to_string();

    Mock::given(method("GET"))
        .and(path("/api/v2/time_entries"))
        .and(query_param("is_running", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "time_entries": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v2/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "projects": [{
                "id": "p1",
                "client": { "id": "c1", "name": "Client A" },
                "name": "Project A",
                "code": "PA",
                "is_active": true,
                "is_billable": true,
                "tasks": [{
                    "id": "t1",
                    "name": "Development",
                    "billable_by_default": true,
                    "is_active": true
                }]
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v2/time_entries"))
        .and(body_json(serde_json::json!({
            "project_id": "p1",
            "task_id": "t1",
            "spent_date": spent_date,
            "is_running": true,
            "source": "cli"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "te_running",
            "project": { "id": "p1", "name": "Project A" },
            "task": { "id": "t1", "name": "Development" },
            "project_id": "p1",
            "task_id": "t1",
            "spent_date": spent_date,
            "hours": 0,
            "duration_seconds": 0,
            "rounded_hours": 0,
            "billable": true,
            "is_running": true,
            "source": "cli",
            "timer_started_at": "2026-09-01T09:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    command_with_mock_config(temp_dir.path(), &server.uri())
        .args([
            "--json",
            "time",
            "start",
            "--project",
            "PA",
            "--task",
            "Development",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status": "started""#));
}

#[tokio::test]
async fn time_running_returns_an_object_when_active() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v2/time_entries"))
        .and(query_param("is_running", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("fixtures/api_v2/time_entry_running.json"),
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let output = command_with_mock_config(temp_dir.path(), &server.uri())
        .args(["--json", "time", "running"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.is_object());
    assert_eq!(value["running"], true);
    assert_eq!(value["entry_id"], "te_running");
}

#[tokio::test]
async fn time_running_returns_an_object_when_inactive() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v2/time_entries"))
        .and(query_param("is_running", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "time_entries": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let output = command_with_mock_config(temp_dir.path(), &server.uri())
        .args(["--json", "time", "running"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.is_object());
    assert_eq!(value["running"], false);
}

#[tokio::test]
async fn time_session_record_update_omits_removed_is_running_field() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v2/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "projects": [{
                "id": "p1",
                "client": { "id": "c1", "name": "Client A" },
                "name": "Project A",
                "code": "PA",
                "is_active": true,
                "is_billable": true,
                "tasks": [{
                    "id": "t1",
                    "name": "Development",
                    "billable_by_default": true,
                    "is_active": true
                }]
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v2/time_entries"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "time_entries": [{
                "id": "te_agent",
                "project_id": "p1",
                "task_id": "t1",
                "spent_date": "2026-09-01",
                "hours": 1,
                "billable": true,
                "is_running": false,
                "source": "agent",
                "metadata": {
                    "skill": "keito-time-track",
                    "session_id": "sess_update",
                    "duration_seconds": 3600
                }
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("PATCH"))
        .and(path("/api/v2/time_entries/te_agent"))
        .and(body_json(serde_json::json!({
            "project_id": "p1",
            "task_id": "t1",
            "spent_date": "2026-09-01",
            "hours": 1.5,
            "metadata": {
                "skill": "keito-time-track",
                "session_id": "sess_update",
                "duration_seconds": 5400
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "te_agent",
            "project": { "id": "p1", "name": "Project A" },
            "task": { "id": "t1", "name": "Development" },
            "project_id": "p1",
            "task_id": "t1",
            "spent_date": "2026-09-01",
            "hours": 1.5,
            "duration_seconds": 5400,
            "rounded_hours": 1.5,
            "billable": true,
            "is_running": false,
            "source": "agent",
            "metadata": {
                "skill": "keito-time-track",
                "session_id": "sess_update",
                "duration_seconds": 5400
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    command_with_mock_config(temp_dir.path(), &server.uri())
        .args([
            "--json",
            "time",
            "session-record",
            "--project",
            "PA",
            "--task",
            "Development",
            "--session-id",
            "sess_update",
            "--duration-seconds",
            "5400",
            "--date",
            "2026-09-01",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status": "updated""#));
}

#[tokio::test]
async fn auth_status_rejects_personal_sync_key_with_actionable_error() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v2/users/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "usr_support",
            "first_name": "Support",
            "last_name": "User",
            "email": "support@keito.ai",
            "is_active": true,
            "is_contractor": false,
            "roles": ["member"],
            "capabilities": {
                "can_read_own_time_entries": true,
                "can_read_users": true,
                "can_read_own_profile": true,
                "can_read_clients": true,
                "can_read_projects": true
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    command_with_mock_config(temp_dir.path(), &server.uri())
        .args(["--json", "auth", "status"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("personal read-only sync key"))
        .stderr(predicate::str::contains("missing field `company`").not());
}

#[tokio::test]
async fn auth_whoami_can_render_personal_sync_identity() {
    let server = MockServer::start().await;
    let temp_dir = tempfile::tempdir().unwrap();
    write_test_config(temp_dir.path(), &server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v2/users/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "usr_support",
            "first_name": "Support",
            "last_name": "User",
            "email": "support@keito.ai",
            "is_active": true,
            "roles": ["member"],
            "capabilities": {
                "can_read_own_time_entries": true,
                "can_read_own_profile": true
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    command_with_mock_config(temp_dir.path(), &server.uri())
        .args(["--json", "auth", "whoami"])
        .assert()
        .success()
        .stdout(predicate::str::contains("support@keito.ai"))
        .stdout(predicate::str::contains(r#""company""#).not());
}

#[test]
fn version_flag_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("keito"));
}
