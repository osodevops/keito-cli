use assert_cmd::Command;
use predicates::prelude::*;

// Acceptance evidence for CAP/MAP/SEC: a missing command is the intentional red
// baseline, not an authentication or fixture failure.
#[test]
fn capture_status_is_a_local_metadata_only_command() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("keito").unwrap()
        .env("KEITO_CAPTURE_HOME", home.path().join("capture"))
        .args(["capture", "status", "--json"])
        .assert().success()
        .stdout(predicate::str::contains("\"pending\": 0"))
        .stdout(predicate::str::contains("\"delivery_verified\": false"));
}

#[test]
fn capture_setup_requires_explicit_local_consent_and_targets() {
    Command::cargo_bin("keito").unwrap()
        .args(["capture", "setup", "--project", "p1", "--task", "t1"])
        .assert().failure()
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
            assert!(!String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap()).contains("PRIVATE"));
        }
    }
}
