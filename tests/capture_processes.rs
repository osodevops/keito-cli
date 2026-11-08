#![cfg(unix)]
use keito_cli::capture::{self, Binding, Clock, Repository, Signal, Store};
use keito_cli::cli::capture::CaptureEvent::Prompt;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn concurrent_real_hook_processes_persist_once_and_discard_raw_content() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("PRIVATE_PATH_CANARY");
    std::fs::create_dir(&repo_root).unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
    let repo = Repository::resolve(&repo_root).unwrap();
    let capture_dir = temp.path().join("capture");
    let mut store = Store::open(&capture_dir).unwrap();
    store
        .approve(Binding {
            repository_key: repo.key.clone(),
            remote_hash: repo.remote_hash.clone(),
            hint_hash: repo.hint_hash.clone(),
            id: String::new(),
            version: 0,
            workspace_id: "co_fixture".into(),
            owner_id: "user_fixture".into(),
            origin_hash: capture::hash("https://example.invalid"),
            project_id: "p_fixture".into(),
            task_id: "t_fixture".into(),
            agents: vec!["codex".into()],
            paused: false,
            revoked: false,
        })
        .unwrap();
    let mut start = Clock::now().unwrap();
    start.wall -= 90;
    start.monotonic -= 90;
    store
        .event(
            &repo,
            Signal {
                source: "codex",
                resumed: false,
                source_id: "session-fixture",
                turn_id: Some("turn-fixture"),
                version: "0.167.0",
                event: Prompt,
                clock: &start,
            },
        )
        .unwrap();
    let body=serde_json::json!({"session_id":"session-fixture","turn_id":"turn-fixture","cwd":repo_root,"prompt":"PRIVATE_PROMPT_CANARY","last_assistant_message":"PRIVATE_ASSISTANT_CANARY","transcript_path":"/PRIVATE_TRANSCRIPT_CANARY","tool_input":{"secret":"PRIVATE_TOOL_CANARY"}}).to_string();
    let children: Vec<_> = (0..20)
        .map(|_| {
            let mut child = Command::new(assert_cmd::cargo::cargo_bin!("keito"))
                .current_dir(&repo_root)
                .env("KEITO_CAPTURE_HOME", &capture_dir)
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
                .write_all(body.as_bytes())
                .unwrap();
            child
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE"));
    }
    let sessions = store.preview("session-fixture").unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].segments.len(), 1);
    assert!((90..=92).contains(&sessions[0].segments[0].duration_seconds));
    assert_eq!(sessions[0].revision, 1);
    for entry in std::fs::read_dir(&capture_dir).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE"));
    }
}

#[test]
fn sqlite_contention_exposes_lost_boundary_as_storage_pause_in_status() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
    let capture = temp.path().join("capture");
    let _store = Store::open(&capture).unwrap();
    let database = rusqlite::Connection::open(capture.join("capture.sqlite3")).unwrap();
    database.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("keito"))
        .current_dir(&root)
        .env("KEITO_CAPTURE_HOME", &capture)
        .args([
            "capture",
            "hook",
            "--agent",
            "codex",
            "--event",
            "prompt",
            "--harness-version",
            "0.167.0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(serde_json::json!({"cwd":root,"session_id":"session-contention","turn_id":"turn-contention","prompt":"PRIVATE_CANARY"}).to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    database.execute_batch("ROLLBACK").unwrap();
    let result = Command::new(assert_cmd::cargo::cargo_bin!("keito"))
        .current_dir(&root)
        .env("KEITO_CAPTURE_HOME", &capture)
        .args(["capture", "status", "--json"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let status: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(status["storage_paused"], true);
    assert!(status["capture_failure"]["boundary_id"].as_str().is_some());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("PRIVATE_CANARY"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("held this event"));
}

#[test]
fn refused_stop_under_real_lock_cannot_be_completed_by_another_hook_process() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
    let repository = Repository::resolve(&root).unwrap();
    let capture = temp.path().join("capture");
    let mut store = Store::open(&capture).unwrap();
    let binding = store
        .approve(Binding {
            repository_key: repository.key.clone(),
            remote_hash: repository.remote_hash.clone(),
            hint_hash: repository.hint_hash.clone(),
            id: String::new(),
            version: 0,
            workspace_id: "co_fixture".into(),
            owner_id: "usr_fixture".into(),
            origin_hash: capture::hash("https://example.invalid"),
            project_id: "p_fixture".into(),
            task_id: "t_fixture".into(),
            agents: vec!["codex".into()],
            paused: false,
            revoked: false,
        })
        .unwrap();
    let mut start = Clock::now().unwrap();
    start.wall -= 90;
    start.monotonic -= 90;
    store
        .event(
            &repository,
            Signal {
                source: "codex",
                resumed: false,
                source_id: "refused-native",
                turn_id: Some("refused-turn"),
                version: "0.167.0",
                event: Prompt,
                clock: &start,
            },
        )
        .unwrap();
    let payload=serde_json::json!({"cwd":root,"session_id":"refused-native","turn_id":"refused-turn","prompt":"PRIVATE_CANARY"}).to_string();
    let database = rusqlite::Connection::open(capture.join("capture.sqlite3")).unwrap();
    let hook = || {
        let mut child = Command::new(assert_cmd::cargo::cargo_bin!("keito"))
            .current_dir(&root)
            .env("KEITO_CAPTURE_HOME", &capture)
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
        child.wait_with_output().unwrap()
    };
    database.execute_batch("BEGIN IMMEDIATE").unwrap();
    let refused = hook();
    assert!(refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("refused this boundary"));
    database.execute_batch("ROLLBACK").unwrap();
    let duplicate = hook();
    assert!(duplicate.status.success());
    let session = &store.preview("refused-native").unwrap()[0];
    assert!(session.needs_attention);
    assert_eq!(session.segments.len(), 1);
    assert_eq!(session.segments[0].duration_seconds, 0);
    assert_eq!(session.segments[0].completeness, "INCOMPLETE");
    assert!(store.status().unwrap().storage_paused);
    let queued = store
        .claim(&binding, chrono::Utc::now().timestamp())
        .unwrap()
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&queued.body).unwrap();
    assert_eq!(body["segments"][0]["duration_seconds"], 0);
    assert_eq!(body["segments"][0]["completeness"], "INCOMPLETE");
    assert!(!queued.body.contains("PRIVATE_CANARY"));
}
