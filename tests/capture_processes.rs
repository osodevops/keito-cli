#![cfg(unix)]
#[path = "support/runtime_pair.rs"]
mod runtime_pair;
#[path = "support/service_boundary.rs"]
mod service_boundary;
use keito_cli::capture::{self, Binding, Clock, Repository, Signal, Store};
use keito_cli::cli::capture::CaptureEvent::Prompt;
use std::io::Write;
use std::process::{Command, Stdio};

#[cfg(feature = "dev-tools")]
#[test]
fn repository_cohort_with_cooperative_parent() {
    use keito_cli::process_ownership::scope::Scope;
    if let Some(root) = std::env::var_os("KEITO_TEST_COHORT_CHILD") {
        let mut scope = Scope::entry_from_environment(500_000_000).unwrap().unwrap();
        let result = keito_cli::source_qa_scope::diagnose_fixture_repository_cohort(
            &mut scope,
            &runtime_pair::pair().core_path,
            std::path::Path::new(&root),
            |_, _, _, _| Ok(()),
        );
        let report = scope.finish();
        scope.publish_terminal(result.is_ok() && report.clean);
        assert!(result.is_ok() && report.clean, "{result:?} {report:?}");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
    let mut now: libc::timespec = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) },
        0
    );
    let end = now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64 + 600_000_000;
    let mut scope = Scope::local_entry(end).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", runtime_pair::process_path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("KEITO_TEST_PAIR_CORE", runtime_pair::pair().core_path)
        .args([
            "--exact",
            "repository_cohort_with_cooperative_parent",
            "--nocapture",
        ])
        .env("KEITO_TEST_COHORT_CHILD", temp.path());
    let slot = scope.start_cooperative(child).unwrap();
    while !scope.slot_complete(&slot) && !scope.failed() {
        scope.poll();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let report = scope.finish();
    assert!(report.clean, "{report:?}");
}

#[cfg(feature = "dev-tools")]
#[test]
fn verified_pair_repository_cohort_fits_the_hook_scope() {
    let temp = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
    let pair = runtime_pair::pair();
    let mut now: libc::timespec = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) },
        0
    );
    let end = now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64 + 500_000_000;
    let mut scope = keito_cli::process_ownership::scope::Scope::local_entry(end).unwrap();
    let result = keito_cli::source_qa_scope::diagnose_fixture_repository_cohort(
        &mut scope,
        &pair.core_path,
        temp.path(),
        |_, _, _, _| Ok(()),
    );
    let report = scope.finish();
    assert!(result.is_ok(), "cohort error={result:?} report={report:?}");
    assert!(report.clean);
}

// Exercise the installed ownership entry rather than the now-refused bare hook.
fn hook(root: &std::path::Path, capture: &std::path::Path, event: &str) -> Command {
    let pair = runtime_pair::pair();
    drop(service_boundary::command(&pair.guard_path));
    let home = capture.parent().unwrap();
    let mut command = Command::new(&pair.guard_path);
    command
        .env_clear()
        .current_dir(root)
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("AppData"))
        .env("PATH", runtime_pair::process_path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("KEITO_CAPTURE_HOME", capture)
        .env("KEITO_CAPTURE_CORE_SHA256", &pair.core_sha256)
        .arg("--core")
        .arg(pair.core_path)
        .args([
            "--agent",
            "codex",
            "--event",
            event,
            "--harness-version",
            "0.167.0",
        ]);
    command
}

#[test]
fn concurrent_real_hook_processes_persist_once_and_discard_raw_content() {
    assert_concurrent_hooks(20, 90);
}

#[test]
fn one_verified_guard_hook_accepts_the_seeded_boundary() {
    assert_concurrent_hooks(1, 90);
}

#[test]
fn guarded_hook_preserves_an_older_seeded_interval_without_a_startup_speed_assumption() {
    assert_concurrent_hooks(1, 93);
}

fn assert_concurrent_hooks(count: usize, seed_age_seconds: i64) {
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
    start.wall -= seed_age_seconds;
    start.monotonic -= seed_age_seconds;
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
    // Bound the real stop clock independently of machine startup speed.
    let earliest_stop_wall = chrono::Utc::now().timestamp();
    let children: Vec<_> = (0..count)
        .map(|_| {
            let mut child = hook(&repo_root, &capture_dir, "stop")
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
    let latest_stop_wall = chrono::Utc::now().timestamp();
    let sessions = store.preview("session-fixture").unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].segments.len(),
        1,
        "synthetic session={} status={}",
        serde_json::to_string(&sessions).unwrap(),
        serde_json::to_string(&store.status().unwrap()).unwrap()
    );
    let segment = &sessions[0].segments[0];
    let segment_start = chrono::DateTime::parse_from_rfc3339(&segment.started_at)
        .unwrap()
        .timestamp();
    let segment_end = chrono::DateTime::parse_from_rfc3339(&segment.ended_at)
        .unwrap()
        .timestamp();
    assert_eq!(
        segment.completeness, "COMPLETE",
        "seconds={} attention={} stop_clock=[{},{}]",
        segment.duration_seconds, sessions[0].needs_attention, earliest_stop_wall, latest_stop_wall
    );
    assert!(!sessions[0].needs_attention);
    assert_eq!(segment_start, start.wall);
    assert!((earliest_stop_wall..=latest_stop_wall).contains(&segment_end));
    assert_eq!(segment.duration_seconds, segment_end - start.wall);
    assert!(
        (earliest_stop_wall - start.wall..=latest_stop_wall - start.wall)
            .contains(&segment.duration_seconds)
    );
    assert_eq!(sessions[0].revision, 1);
    assert_no_private_content(&capture_dir);
}

fn assert_no_private_content(directory: &std::path::Path) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(!metadata.file_type().is_symlink(), "fixture tree escaped");
        if metadata.is_dir() {
            assert_no_private_content(&path);
        } else {
            assert!(metadata.is_file());
            let bytes = std::fs::read(path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE"));
        }
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
    let mut child = hook(&root, &capture, "prompt")
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
    let result = Command::new(runtime_pair::pair().core_path)
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
    assert_refused_stop(false);
}

#[test]
fn refused_stop_survives_a_duplicate_that_is_also_refused_before_sqlite_admission() {
    assert_refused_stop(true);
}

fn assert_refused_stop(duplicate_contended: bool) {
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
    let invoke_hook = || {
        let mut child = hook(&root, &capture, "stop")
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
    let refused = invoke_hook();
    assert!(refused.status.success());
    assert!(refused.stderr.is_empty(), "installed guard stays silent");
    assert!(
        store.status().unwrap().storage_paused,
        "refusal must be durable"
    );
    if !duplicate_contended {
        database.execute_batch("ROLLBACK").unwrap();
    }
    let duplicate = invoke_hook();
    assert!(duplicate.status.success());
    if duplicate_contended {
        database.execute_batch("ROLLBACK").unwrap();
    }
    // Guard success is silent/fail-open, so the duplicate may itself have
    // refused before reaching SQLite. Never allow it to manufacture time,
    // even temporarily, before the normal transmission gate reconciles it.
    let before_claim = store.preview("refused-native").unwrap();
    assert!(before_claim[0]
        .segments
        .iter()
        .all(|segment| segment.completeness != "COMPLETE" || segment.duration_seconds == 0));
    assert!(store.status().unwrap().storage_paused);
    assert!(
        store
            .claim(&binding, chrono::Utc::now().timestamp())
            .unwrap()
            .is_none(),
        "a retained failed invocation must block transmission"
    );
    let session = &store.preview("refused-native").unwrap()[0];
    assert!(session.needs_attention);
    assert_eq!(session.segments.len(), 1);
    assert_eq!(session.segments[0].duration_seconds, 0);
    assert_eq!(session.segments[0].completeness, "INCOMPLETE");
    assert!(store.status().unwrap().storage_paused);
    store.recover(&Clock::now().unwrap()).unwrap();
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
