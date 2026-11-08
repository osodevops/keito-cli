//! Real adapter/process safety proof with a deliberately never-entered core.
//! This is not native harness, first-artifact latency or financial qualification.
#![cfg(unix)]
use keito_cli::capture::{self, Binding, Clock, Repository, Signal, Store};
use keito_cli::cli::capture::CaptureEvent::Prompt;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};
#[test]
fn watchdog_kill_before_core_entry_leaves_durable_flag_and_later_real_stop_is_incomplete() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    fs::create_dir(&root).unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
    let repo = Repository::resolve(&root).unwrap();
    let directory = base.join("capture");
    let mut store = Store::open(&directory).unwrap();
    store
        .approve(Binding {
            repository_key: repo.key.clone(),
            remote_hash: repo.remote_hash.clone(),
            hint_hash: repo.hint_hash.clone(),
            id: String::new(),
            version: 0,
            workspace_id: "co_test".into(),
            owner_id: "owner".into(),
            origin_hash: capture::hash("origin"),
            project_id: "p".into(),
            task_id: "t".into(),
            agents: vec!["codex".into()],
            paused: false,
            revoked: false,
        })
        .unwrap();
    let mut opened = Clock::now().unwrap();
    opened.wall -= 90;
    opened.monotonic -= 90;
    store
        .event(
            &repo,
            Signal {
                source: "codex",
                resumed: false,
                source_id: "native",
                turn_id: Some("turn"),
                version: "0.167.0",
                event: Prompt,
                clock: &opened,
            },
        )
        .unwrap();
    let slow = base.join("never-enters-core");
    fs::write(&slow, "#!/bin/sh\nsleep 5\nexit 0\n").unwrap();
    fs::set_permissions(&slow, fs::Permissions::from_mode(0o700)).unwrap();
    let payload=serde_json::json!({"cwd":root,"session_id":"native","turn_id":"turn","prompt":"PRIVATE_RAW_GUARD_CANARY","transcript_path":"/PRIVATE_RAW_GUARD_PATH"}).to_string();
    let started = std::time::Instant::now();
    let mut guard = Command::new("bash")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/skills/keito-time-track/hooks/dispatch.sh"
        ))
        .args([
            "--agent",
            "codex",
            "--event",
            "stop",
            "--harness-version",
            "0.167.0",
        ])
        .env("KEITO_CAPTURE_HOME", &directory)
        .env("KEITO_CLI_BIN", &slow)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    guard
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let output = guard.wait_with_output().unwrap();
    let elapsed = started.elapsed();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "actual guard exceeded host deadline: {elapsed:?}"
    );
    assert!(fs::read_dir(directory.join("capture-invocations"))
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending")));
    // Direct actual CLI reconciliation, with no warming or success claim for the
    // failed invocation. An old runtime would invent about90 complete seconds.
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("keito"))
        .current_dir(&root)
        .env("KEITO_CAPTURE_HOME", &directory)
        .args([
            "capture",
            "hook",
            "--agent",
            "codex",
            "--event",
            "stop",
            "--harness-version",
            "0.167.0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    let session = &store.preview("native").unwrap()[0];
    assert_eq!(session.segments[0].duration_seconds, 0);
    assert_eq!(session.segments[0].completeness, "INCOMPLETE");
    assert!(session.needs_attention);
    assert!(store.status().unwrap().storage_paused);
    for entry in fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = fs::read(path).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains("PRIVATE_RAW_GUARD_CANARY"));
            assert!(!text.contains("PRIVATE_RAW_GUARD_PATH"));
        }
    }
}
