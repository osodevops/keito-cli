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
    // Hold the independent authority lock as well as real SQLite contention:
    // refusal publication must use its nonblocking immutable-marker fallback.
    let mut authority=Command::new("python3").args(["-c","import fcntl,sys; f=open(sys.argv[1],'a+'); fcntl.flock(f,fcntl.LOCK_EX); print('locked',flush=True); sys.stdin.read()"])
        .arg(capture.join("capture-authority.lock")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    use std::io::BufRead;
    let mut ready = String::new();
    std::io::BufReader::new(authority.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "locked");
    let database = rusqlite::Connection::open(capture.join("capture.sqlite3")).unwrap();
    database.execute_batch("BEGIN IMMEDIATE").unwrap();
    // The core admission+durable marker bound is independent of OS first-launch
    // admission before main. The actual CLI path below remains a separate safety
    // check; its first-launch timing is retained as qualification evidence.
    let mut probe = Store::open(&capture).unwrap();
    let diagnostic_repo = Repository::resolve(&root).unwrap();
    let diagnostic_clock = Clock::now().unwrap();
    let core_start = std::time::Instant::now();
    assert!(probe
        .event(
            &diagnostic_repo,
            Signal {
                source: "codex",
                resumed: false,
                source_id: "core-contention",
                turn_id: Some("turn"),
                version: "0.167.0",
                event: Prompt,
                clock: &diagnostic_clock
            }
        )
        .is_err());
    let event_elapsed = core_start.elapsed();
    #[cfg(feature = "dev-tools")]
    let admission = probe.last_admission_timing().unwrap().clone();
    let refusal_start = std::time::Instant::now();
    capture::record_failure(
        &capture,
        &capture::hash("core-contention"),
        "STORAGE_UNAVAILABLE",
    )
    .unwrap();
    let refusal_elapsed = refusal_start.elapsed();
    let core_elapsed = core_start.elapsed();
    println!(
        "contention timings: event={:.3}ms refusal={:.3}ms total={:.3}ms",
        event_elapsed.as_secs_f64() * 1000.0,
        refusal_elapsed.as_secs_f64() * 1000.0,
        core_elapsed.as_secs_f64() * 1000.0
    );
    #[cfg(feature = "dev-tools")]
    {
        println!(
            "admission phases: permit={:?} sqlite_begin={:?} sqlite_error={:?} authority={:?}",
            admission.permit_validation,
            admission.sqlite_begin,
            admission.sqlite_error,
            admission.authority
        );
        assert!(admission.permit_validation.is_some());
        assert!(admission.sqlite_begin.is_some());
        assert_eq!(
            admission.sqlite_error,
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
        assert_eq!(
            admission.authority, None,
            "authority must not start after failed SQLite admission"
        );
    }
    assert!(
        core_elapsed < std::time::Duration::from_millis(700),
        "core admission and marker exceeded watchdog budget: event={event_elapsed:?} refusal={refusal_elapsed:?} total={core_elapsed:?}"
    );
    drop(probe);
    let start = std::time::Instant::now();
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
    let elapsed = start.elapsed();
    println!(
        "combined SQLite/authority refusal completed in {}ms",
        elapsed.as_millis()
    );
    authority.kill().unwrap();
    authority.wait().unwrap();
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

#[test]
fn authority_lock_contention_never_blocks_refusal_publication_and_kill_releases_lock() {
    use std::io::BufRead;
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("capture");
    let mut store = Store::open(&directory).unwrap();
    let mut child=Command::new("python3").args(["-c","import fcntl,sys; f=open(sys.argv[1],'a+'); fcntl.flock(f,fcntl.LOCK_EX); print('locked',flush=True); sys.stdin.read()"])
        .arg(directory.join("capture-authority.lock")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut ready = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "locked");
    let start = std::time::Instant::now();
    capture::record_failure(
        &directory,
        &capture::hash("refused-with-authority-held"),
        "STORAGE_UNAVAILABLE",
    )
    .unwrap();
    assert!(
        start.elapsed() < std::time::Duration::from_millis(100),
        "refusal publication added an authority wait"
    );
    let clock = Clock::now().unwrap();
    assert!(store.recover(&clock).is_err());
    assert!(store.status().unwrap().storage_paused);
    child.kill().unwrap();
    child.wait().unwrap();
    store.recover(&clock).unwrap();
    assert!(!store.status().unwrap().storage_paused);
}

#[test]
fn distinct_refusals_are_immutable_and_recovery_cannot_confuse_replayed_boundary_ids() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let boundary = capture::hash("same-boundary");
    for _ in 0..4 {
        capture::record_failure(temp.path(), &boundary, "BOUNDARY_REFUSED").unwrap();
    }
    let count = std::fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("capture-refusal-")
        })
        .count();
    assert_eq!(count, 4);
    store.recover(&Clock::now().unwrap()).unwrap();
    assert!(!store.status().unwrap().storage_paused);
}
