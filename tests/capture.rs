use assert_cmd::Command;
use predicates::prelude::*;

// Acceptance evidence for CAP/MAP/SEC: a missing command is the intentional red
// baseline, not an authentication or fixture failure.
#[test]
fn capture_status_is_a_local_metadata_only_command() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("keito")
        .unwrap()
        .env("KEITO_CAPTURE_HOME", home.path().join("capture"))
        .args(["capture", "status", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pending\": 0"))
        .stdout(predicate::str::contains("\"delivery_verified\": false"));
}

#[test]
fn capture_setup_requires_explicit_local_consent_and_targets() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["capture", "setup", "--project", "p1", "--task", "t1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--agent"));
}

#[test]
fn capture_hook_discards_sensitive_input_without_a_mapping() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("keito").unwrap()
        .env("KEITO_CAPTURE_HOME", home.path().join("capture"))
        .args(["capture", "hook", "--agent", "codex", "--event", "stop", "--harness-version", "fixture-v1"])
        .write_stdin(r#"{"session_id":"synthetic-session","cwd":"/does-not-exist/PRIVATE_PATH_CANARY","prompt":"PRIVATE_PROMPT_CANARY","last_assistant_message":"PRIVATE_ASSISTANT_CANARY"}"#)
        .assert().success().stdout("")
        .stderr(predicate::str::contains("PRIVATE").not());
    for entry in std::fs::read_dir(home.path()).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_file() {
            assert!(
                !String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap()).contains("PRIVATE")
            );
        }
    }
}

#[test]
fn scripted_skill_install_without_targets_has_no_side_effects() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("keito")
        .unwrap()
        .env("HOME", home.path())
        .args(["skill", "install", "--json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--agent"));
    assert!(!home.path().join(".codex").exists());
    assert!(!home.path().join(".claude").exists());
}

#[cfg(unix)]
#[test]
fn scripted_source_setup_without_targets_stops_before_installation() {
    let home = tempfile::tempdir().unwrap();
    Command::new("bash")
        .arg("setup")
        .env("HOME", home.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--host"));
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn replaced_worker_runtime_is_refused_before_loading_credentials_or_network() {
    Command::cargo_bin("keito")
        .unwrap()
        .args([
            "capture",
            "worker",
            "--expected-runtime-hash",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("runtime changed"));
}

#[test]
fn live_doctor_malformed_config_never_echoes_credential_source_lines() {
    let home=tempfile::tempdir().unwrap();
    for directory in [home.path().join("Library/Application Support/keito"),home.path().join("config/keito"),home.path().join("AppData/Roaming/keito")] {
        std::fs::create_dir_all(&directory).unwrap();std::fs::write(directory.join("config.toml"),"api_key = \"kto_PRIVATE_CONFIG_CANARY\" invalid\n").unwrap();
    }
    Command::cargo_bin("keito").unwrap().env("HOME",home.path()).env("XDG_CONFIG_HOME",home.path().join("config")).env("APPDATA",home.path().join("AppData/Roaming"))
        .args(["skill","doctor","--live","--json"]).assert().failure().stderr(predicate::str::contains("PRIVATE_CONFIG_CANARY").not());
}
