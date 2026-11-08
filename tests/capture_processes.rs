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
