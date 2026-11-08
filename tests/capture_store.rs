use keito_cli::capture::{self, Binding, Clock, Repository, Store};
use keito_cli::cli::capture::CaptureEvent::*;
use std::path::Path;

fn clock(s: i64) -> Clock {
    Clock {
        wall: 1_794_067_200 + s,
        monotonic: 100_000 + s,
        boot: "fixture-boot".into(),
    }
}
fn repo() -> Repository {
    Repository {
        key: capture::hash("fixture-repo"),
        hint_hash: capture::hash("fixture-config"),
        disabled: false,
    }
}
fn binding() -> Binding {
    Binding {
        repository_key: repo().key,
        hint_hash: repo().hint_hash,
        id: String::new(),
        version: 0,
        workspace_id: "co_fixture".into(),
        owner_id: "user_fixture".into(),
        origin_hash: capture::hash("http://fixture"),
        project_id: "project_fixture".into(),
        task_id: "task_fixture".into(),
        agents: vec!["codex".into(), "claude-code".into()],
        paused: false,
        revoked: false,
    }
}
fn setup() -> (tempfile::TempDir, Store, Binding) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("capture")).unwrap();
    let b = store.approve(binding()).unwrap();
    (dir, store, b)
}
fn event(
    store: &mut Store,
    session: &str,
    turn: Option<&str>,
    event: keito_cli::cli::capture::CaptureEvent,
    s: i64,
) {
    store
        .event(
            &repo(),
            capture::Signal {
                source: "codex",
                source_id: session,
                turn_id: turn,
                version: "fixture-v1",
                event,
                clock: &clock(s),
            },
        )
        .unwrap()
}
fn body(store: &mut Store, b: &Binding, s: i64) -> (capture::Queued, serde_json::Value) {
    let queued = store.claim(b, clock(s).wall).unwrap().unwrap();
    let body = serde_json::from_str(&queued.body).unwrap();
    (queued, body)
}

#[test]
fn three_turns_short_first_idle_and_resume_preserve_exact_seconds() {
    let (_dir, mut store, b) = setup();
    event(&mut store, "session", None, SessionStart, 0);
    event(&mut store, "session", Some("turn-1"), Prompt, 0);
    event(&mut store, "session", Some("turn-1"), Stop, 1);
    event(&mut store, "session", None, SessionStart, 100);
    event(&mut store, "session", Some("turn-2"), Prompt, 100);
    event(&mut store, "session", Some("turn-2"), Stop, 159);
    event(&mut store, "session", Some("turn-3"), Prompt, 500);
    event(&mut store, "session", Some("turn-3"), Stop, 590);
    let session = store.preview("session").unwrap().remove(0);
    assert_eq!(
        session
            .segments
            .iter()
            .map(|s| s.duration_seconds)
            .collect::<Vec<_>>(),
        vec![1, 59, 90]
    );
    assert_eq!(session.revision, 3);
    let (q, payload) = body(&mut store, &b, 600);
    assert_eq!(payload["segments"][0]["duration_seconds"], 1);
    assert_eq!(payload["expected_revision"], 0);
    assert!(!q.body.contains("PRIVATE"));
    assert!(payload.get("owner_id").is_none());
    assert!(payload.get("billable").is_none());
}

#[test]
fn frozen_failure_retries_do_not_count_upload_wait_and_preserve_identity() {
    let (_dir, mut store, b) = setup();
    event(&mut store, "session", Some("turn"), Prompt, 0);
    event(&mut store, "session", Some("turn"), Stop, 90);
    let (q, first) = body(&mut store, &b, 91);
    store
        .finish(&q, "pending", "NETWORK_UNAVAILABLE", clock(91).wall, None)
        .unwrap();
    let (retry, second) = body(&mut store, &b, 391);
    assert_eq!(q.body, retry.body);
    assert_eq!(q.hash, retry.hash);
    assert_eq!(first, second);
    assert_eq!(second["segments"][0]["duration_seconds"], 90);
}

#[test]
fn missing_or_wrong_session_never_uses_latest_directory_state() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "session-b", Some("turn-b"), Prompt, 0);
    event(&mut store, "session-a", Some("turn-a"), Stop, 100);
    assert!(store.preview("session-a").unwrap().is_empty());
    event(&mut store, "session-b", Some("turn-b"), Stop, 59);
    assert_eq!(
        store.preview("session-b").unwrap()[0].segments[0].duration_seconds,
        59
    );
}

#[test]
fn duplicate_boundaries_are_idempotent_and_out_of_order_stop_cannot_close_new_turn() {
    let (_dir, mut store, _) = setup();
    for _ in 0..3 {
        event(&mut store, "session", Some("turn-1"), Prompt, 0);
    }
    event(&mut store, "session", Some("turn-1"), Stop, 60);
    event(&mut store, "session", Some("turn-1"), Prompt, 61);
    event(&mut store, "session", Some("turn-2"), Prompt, 100);
    event(&mut store, "session", Some("turn-1"), Stop, 110);
    event(&mut store, "session", Some("turn-2"), Stop, 190);
    for _ in 0..3 {
        event(&mut store, "session", Some("turn-2"), Stop, 191);
    }
    let s = &store.preview("session").unwrap()[0];
    assert_eq!(s.segments.len(), 2);
    assert_eq!(
        s.segments.iter().map(|s| s.duration_seconds).sum::<i64>(),
        150
    );
    assert_eq!(s.revision, 2);
}

#[test]
fn pause_excludes_interval_and_does_not_reset_earned_time() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "session", Some("turn"), Prompt, 0);
    store.control(&repo(), "pause", &clock(30)).unwrap();
    event(&mut store, "session", Some("turn"), Stop, 100);
    store.control(&repo(), "resume", &clock(130)).unwrap();
    event(&mut store, "session", Some("turn"), Stop, 175);
    let s = &store.preview("session").unwrap()[0];
    assert_eq!(
        s.segments.iter().map(|s| s.duration_seconds).sum::<i64>(),
        75
    );
    assert_eq!(s.segments.len(), 2);
}

#[test]
fn interrupt_missing_stop_and_reboot_never_invent_complete_time() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "session", Some("turn-1"), Prompt, 0);
    event(&mut store, "session", Some("turn-2"), Prompt, 300);
    event(&mut store, "session", Some("turn-2"), Interrupt, 400);
    event(&mut store, "session", Some("turn-3"), Prompt, 500);
    let mut reboot = clock(590);
    reboot.boot = "other-boot".into();
    store
        .event(
            &repo(),
            capture::Signal {
                source: "codex",
                source_id: "session",
                turn_id: Some("turn-3"),
                version: "fixture-v1",
                event: Stop,
                clock: &reboot,
            },
        )
        .unwrap();
    event(&mut store, "session", None, SessionEnd, 600);
    let s = &store.preview("session").unwrap()[0];
    assert!(s.needs_attention);
    assert_eq!(
        s.segments.iter().map(|s| s.duration_seconds).sum::<i64>(),
        0
    );
    assert!(s.segments.iter().all(|s| s.completeness == "INCOMPLETE"));
}

#[test]
fn backward_clock_and_implausible_intervals_are_incomplete() {
    for end in [-1, 100_000] {
        let (_dir, mut store, _) = setup();
        event(&mut store, "session", Some("turn"), Prompt, 0);
        event(&mut store, "session", Some("turn"), Stop, end);
        let s = &store.preview("session").unwrap()[0];
        assert!(s.needs_attention);
        assert_eq!(s.segments[0].duration_seconds, 0);
    }
}

#[test]
fn no_native_turn_identity_is_a_refusal_not_fabricated_elapsed_time() {
    let (_dir, mut store, _) = setup();
    assert!(store
        .event(
            &repo(),
            capture::Signal {
                source: "claude-code",
                source_id: "session",
                turn_id: None,
                version: "fixture-v1",
                event: Prompt,
                clock: &clock(0)
            }
        )
        .is_err());
    assert!(store.preview("session").unwrap().is_empty());
}

#[test]
fn changed_mapping_or_revoked_consent_holds_new_collection_and_pending_delivery() {
    let (_dir, mut store, b) = setup();
    event(&mut store, "session", Some("turn"), Prompt, 0);
    event(&mut store, "session", Some("turn"), Stop, 60);
    let mut changed = repo();
    changed.hint_hash = capture::hash("other-config");
    store
        .event(
            &changed,
            capture::Signal {
                source: "codex",
                source_id: "new-session",
                turn_id: Some("turn"),
                version: "fixture-v1",
                event: Prompt,
                clock: &clock(70),
            },
        )
        .unwrap();
    assert!(store.preview("new-session").unwrap().is_empty());
    store.control(&repo(), "disable", &clock(70)).unwrap();
    assert!(store.claim(&b, clock(71).wall).unwrap().is_none());
    assert!(store.control(&repo(), "resume", &clock(72)).is_err());
    assert_eq!(store.status().unwrap().pending, 1);
}

#[test]
fn next_revision_is_not_claimed_until_previous_acknowledgement() {
    let (_dir, mut store, b) = setup();
    for n in 0..2 {
        let turn = format!("turn-{n}");
        event(&mut store, "session", Some(&turn), Prompt, n * 100);
        event(&mut store, "session", Some(&turn), Stop, n * 100 + 90);
    }
    let (q, p) = body(&mut store, &b, 300);
    assert_eq!(p["revision"], 1);
    assert!(store.claim(&b, clock(301).wall).unwrap().is_none());
    store
        .finish(&q, "acknowledged", "ACCEPTED", clock(301).wall, None)
        .unwrap();
    let (_, p) = body(&mut store, &b, 302);
    assert_eq!(p["revision"], 2);
    assert_eq!(p["expected_revision"], 1);
}

#[test]
fn sqlite_connections_serialize_duplicate_hooks_into_one_revision() {
    let (dir, mut store, b) = setup();
    event(&mut store, "session", Some("turn"), Prompt, 0);
    let dir = dir.path().join("capture");
    let handles: Vec<_> = (0..20)
        .map(|_| {
            let path = dir.clone();
            std::thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                event(&mut store, "session", Some("turn"), Stop, 90);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let s = &store.preview("session").unwrap()[0];
    assert_eq!(s.segments.len(), 1);
    assert_eq!(s.revision, 1);
    let (_, p) = body(&mut store, &b, 200);
    assert_eq!(p["segments"][0]["duration_seconds"], 90);
}

#[cfg(unix)]
#[test]
fn queue_has_private_modes_and_rejects_symlinks() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (dir, _store, _) = setup();
    let path = dir.path().join("capture");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(path.join("capture.sqlite3"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let link = dir.path().join("link");
    symlink(&path, &link).unwrap();
    assert!(Store::open(&link).is_err());
}

#[test]
fn credential_and_path_identifiers_reject_without_echo() {
    for value in [
        "/Users/private",
        "../secrets",
        "kto_SECRET_CANARY",
        "sk-secret",
        "ghp_SECRET_CANARY",
        "AKIASECRETCANARY",
    ] {
        let error = capture::identifier(value).unwrap_err().to_string();
        assert!(!error.contains(value));
    }
}

fn git(path: &Path, args: &[&str]) {
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap()
        .status
        .success())
}
#[test]
fn linked_worktrees_share_identity_but_nested_repositories_and_changed_remotes_do_not() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init"]);
    git(
        &root,
        &[
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "user.name=Fixture",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
    let linked = temp.path().join("linked");
    git(
        &root,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let original = Repository::resolve(&root).unwrap();
    assert_eq!(original.key, Repository::resolve(&linked).unwrap().key);
    let sub = root.join("sub");
    std::fs::create_dir(&sub).unwrap();
    assert_eq!(original.key, Repository::resolve(&sub).unwrap().key);
    git(&sub, &["init"]);
    assert_ne!(original.key, Repository::resolve(&sub).unwrap().key);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://credential:SECRET_CANARY@example.invalid/repo",
        ],
    );
    let changed = Repository::resolve(&root).unwrap();
    assert_ne!(original.key, changed.key);
    assert!(!changed.key.contains("SECRET"));
}
