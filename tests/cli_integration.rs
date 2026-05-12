#![allow(deprecated)]
use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use wiremock::matchers::{body_json, header, method, path};
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

fn command_with_mock_config(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("keito").unwrap();
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("AppData").join("Roaming"))
        .env("KEITO_API_KEY", "kto_test_key")
        .env("KEITO_ACCOUNT_ID", "co_test")
        .env_remove("KEITO_WORKSPACE_ID");
    cmd
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

    command_with_mock_config(temp_dir.path())
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
fn whoami_without_auth_fails() {
    let temp_dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("keito")
        .unwrap()
        .args(["auth", "whoami"])
        .env("HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path().join("config"))
        .env_remove("KEITO_API_KEY")
        .env_remove("KEITO_ACCOUNT_ID")
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
                "skill": "keito-agent"
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

    command_with_mock_config(temp_dir.path())
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
            "keito-agent",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status": "created""#))
        .stdout(predicate::str::contains(r#""session_id": "sess_123""#));
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
