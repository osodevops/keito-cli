#![allow(deprecated)]
use assert_cmd::Command;
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
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("AppData").join("Roaming"))
        .env("KEITO_API_URL", api_url)
        .env("KEITO_API_KEY", "kto_test_key")
        .env("KEITO_ACCOUNT_ID", "co_test")
        .env_remove("KEITO_WORKSPACE_ID");
    cmd
}

#[cfg(unix)]
fn prepend_path(cmd: &mut Command, dir: &Path) {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(current_path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current_path));
    }
    cmd.env("PATH", std::env::join_paths(paths).unwrap());
}

#[cfg(unix)]
fn write_fake_skill_tools(home: &Path) -> std::path::PathBuf {
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();

    let npx = bin.join("npx");
    fs::write(
        &npx,
        r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$KEITO_FAKE_NPX_LOG"
echo "fake npx install output"
agent=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-a" ]; then
    shift
    agent="${1:-}"
  fi
  shift || true
done

case "$agent" in
  codex)
    root="$HOME/.agents/skills/keito-time-track"
    mkdir -p "$root/installers"
    cat > "$root/installers/install-codex.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
echo "fake codex installer output"
mkdir -p "$HOME/.codex"
printf '{"hooks":{"SessionStart":[{"hooks":[{"command":"keito-time-track/hooks/session-start.sh"}]}],"Stop":[{"hooks":[{"command":"keito-time-track/hooks/session-end.sh"}]}]}}\n' > "$HOME/.codex/hooks.json"
SH
    chmod +x "$root/installers/install-codex.sh"
    ;;
  claude-code)
    root="$HOME/.claude/skills/keito-time-track"
    mkdir -p "$root/installers"
    cat > "$root/installers/install-claude-code.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
echo "fake claude installer output"
mkdir -p "$HOME/.claude"
printf '{"hooks":{"SessionStart":[{"hooks":[{"command":"keito-time-track/hooks/session-start.sh"}]}],"Stop":[{"hooks":[{"command":"keito-time-track/hooks/session-end.sh"}]}]}}\n' > "$HOME/.claude/settings.json"
SH
    chmod +x "$root/installers/install-claude-code.sh"
    ;;
  *)
    echo "unexpected agent: $agent" >&2
    exit 2
    ;;
esac
"#,
    )
    .unwrap();
    set_executable(&npx);

    let jq = bin.join("jq");
    fs::write(&jq, "#!/usr/bin/env bash\nexit 0\n").unwrap();
    set_executable(&jq);

    bin
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
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
fn skill_install_configures_agent_hooks_with_fake_skills_cli() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bin = write_fake_skill_tools(temp_dir.path());
    let npx_log = temp_dir.path().join("npx.log");

    let mut cmd = Command::cargo_bin("keito").unwrap();
    prepend_path(&mut cmd, &bin);
    let output = cmd
        .env("HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path().join("config"))
        .env("APPDATA", temp_dir.path().join("AppData").join("Roaming"))
        .env("KEITO_FAKE_NPX_LOG", &npx_log)
        .env("KEITO_API_KEY", "kto_test_key")
        .env("KEITO_ACCOUNT_ID", "co_test")
        .args([
            "--json",
            "skill",
            "install",
            "--source",
            "/tmp/keito-skill",
            "--agent",
            "codex",
            "--agent",
            "claude-code",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let status_json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(status_json["authenticated"], true);
    assert_eq!(status_json["codex"]["hooks_configured"], true);
    assert_eq!(status_json["claude_code"]["hooks_configured"], true);

    let output_text = String::from_utf8(output).unwrap();
    assert!(!output_text.contains("fake npx install output"));
    assert!(!output_text.contains("fake codex installer output"));
    assert!(!output_text.contains("fake claude installer output"));

    let log = fs::read_to_string(npx_log).unwrap();
    assert!(log.contains("skills@1.5.6 add /tmp/keito-skill"));
    assert!(log.contains("-a codex"));
    assert!(log.contains("-a claude-code"));
    assert!(temp_dir.path().join(".codex/hooks.json").exists());
    assert!(temp_dir.path().join(".claude/settings.json").exists());
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
