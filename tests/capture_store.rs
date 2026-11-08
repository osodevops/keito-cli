use keito_cli::capture::{self, Repository};
#[cfg(unix)]
use keito_cli::capture::{Binding, Clock, Store};
#[cfg(unix)]
use keito_cli::cli::capture::CaptureEvent::*;
use std::path::Path;

#[cfg(unix)]
fn clock(s: i64) -> Clock {
    Clock {
        wall: 1_794_067_200 + s,
        monotonic: 100_000 + s,
        boot: "fixture-boot".into(),
    }
}
#[cfg(unix)]
fn repo() -> Repository {
    Repository {
        key: capture::hash("fixture-repo"),
        remote_hash: capture::hash("fixture-remote"),
        linked_without_hint: false,
        hint_hash: capture::hash("fixture-config"),
        disabled: false,
    }
}
#[cfg(unix)]
fn binding() -> Binding {
    Binding {
        repository_key: repo().key,
        remote_hash: repo().remote_hash,
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
#[cfg(unix)]
fn setup() -> (tempfile::TempDir, Store, Binding) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("capture")).unwrap();
    let b = store.approve(binding()).unwrap();
    (dir, store, b)
}
#[cfg(unix)]
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
                resumed: false,
                source_id: session,
                turn_id: turn,
                version: "fixture-v1",
                event,
                clock: &clock(s),
            },
        )
        .unwrap()
}
#[cfg(unix)]
fn body(store: &mut Store, b: &Binding, s: i64) -> (capture::Queued, serde_json::Value) {
    let queued = store.claim(b, clock(s).wall).unwrap().unwrap();
    let body = serde_json::from_str(&queued.body).unwrap();
    (queued, body)
}

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn pause_excludes_interval_and_does_not_reset_earned_time() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "session", Some("turn"), Prompt, 0);
    store.control(&repo(), "pause", &clock(30)).unwrap();
    store.control(&repo(), "resume", &clock(130)).unwrap();
    event(&mut store, "session", Some("turn"), Stop, 175);
    let s = &store.preview("session").unwrap()[0];
    assert_eq!(
        s.segments.iter().map(|s| s.duration_seconds).sum::<i64>(),
        75
    );
    assert_eq!(s.segments.len(), 2);
}

#[cfg(unix)]
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
                resumed: false,
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

#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn no_native_turn_identity_is_a_refusal_not_fabricated_elapsed_time() {
    let (_dir, mut store, _) = setup();
    assert!(store
        .event(
            &repo(),
            capture::Signal {
                source: "claude-code",
                resumed: false,
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

#[cfg(unix)]
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
                resumed: false,
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
    assert_eq!(store.status().unwrap().pending, 0);
    assert_eq!(store.status().unwrap().held, 1);
}

#[cfg(unix)]
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

#[cfg(unix)]
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
    assert_eq!(original.key, changed.key);
    assert_ne!(original.remote_hash, changed.remote_hash);
    assert!(!changed.key.contains("SECRET"));
}

#[cfg(unix)]
#[test]
fn repeated_resume_does_not_reset_an_active_interval() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "session", Some("turn"), Prompt, 0);
    store.control(&repo(), "resume", &clock(30)).unwrap();
    store.control(&repo(), "resume", &clock(45)).unwrap();
    event(&mut store, "session", Some("turn"), Stop, 90);
    assert_eq!(
        store.preview("session").unwrap()[0].segments[0].duration_seconds,
        90
    );
}
#[cfg(unix)]
#[test]
fn repeated_setup_preserves_approval_identity_and_pause() {
    let (_dir, mut store, b) = setup();
    store.control(&repo(), "pause", &clock(0)).unwrap();
    let same = store.approve(binding()).unwrap();
    assert_eq!(same.id, b.id);
    assert_eq!(same.version, b.version);
    assert!(same.paused);
}
#[cfg(unix)]
#[test]
fn old_completed_prompt_replay_cannot_close_or_duplicate_a_newer_turn() {
    let (_dir, mut store, _) = setup();
    for n in 1..=2 {
        let id = format!("turn-{n}");
        event(&mut store, "session", Some(&id), Prompt, n * 100);
        event(&mut store, "session", Some(&id), Stop, n * 100 + 90);
    }
    event(&mut store, "session", Some("turn-3"), Prompt, 300);
    event(&mut store, "session", Some("turn-1"), Prompt, 310);
    event(&mut store, "session", Some("turn-3"), Stop, 390);
    assert_eq!(
        store.preview("session").unwrap()[0]
            .segments
            .iter()
            .map(|s| s.duration_seconds)
            .sum::<i64>(),
        270
    );
}

#[cfg(unix)]
#[test]
fn resumed_harness_keeps_completed_turns_and_marks_the_abandoned_interval_incomplete() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "session", Some("first"), Prompt, 0);
    event(&mut store, "session", Some("first"), Stop, 90);
    event(&mut store, "session", Some("abandoned"), Prompt, 100);
    store
        .event(
            &repo(),
            capture::Signal {
                source: "codex",
                resumed: true,
                source_id: "session",
                turn_id: None,
                version: "0.167.0",
                event: SessionStart,
                clock: &clock(1000),
            },
        )
        .unwrap();
    let s = &store.preview("session").unwrap()[0];
    assert_eq!(s.segments.len(), 2);
    assert_eq!(s.segments[0].duration_seconds, 90);
    assert_eq!(s.segments[1].duration_seconds, 0);
    assert_eq!(s.segments[1].completeness, "INCOMPLETE");
    assert!(s.needs_attention);
}

#[cfg(unix)]
#[test]
fn stopped_while_paused_never_reopens_or_counts_idle_after_resume() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "paused-end", Some("turn"), Prompt, 0);
    store.control(&repo(), "pause", &clock(30)).unwrap();
    event(&mut store, "paused-end", Some("turn"), Stop, 60);
    store.control(&repo(), "resume", &clock(100)).unwrap();
    event(&mut store, "paused-end", Some("turn"), Stop, 150);
    assert_eq!(
        store.preview("paused-end").unwrap()[0]
            .segments
            .iter()
            .map(|s| s.duration_seconds)
            .sum::<i64>(),
        30
    );
    assert_eq!(store.status().unwrap().unfinished, 0);
    assert_eq!(store.status().unwrap().pending, 1);
}
#[cfg(unix)]
#[test]
fn adding_harness_preserves_native_session_revision_and_identical_reenable_keeps_identity() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(&temp.path().join("capture")).unwrap();
    let mut b = binding();
    b.agents = vec!["codex".into()];
    let first = store.approve(b).unwrap();
    event(&mut store, "native", Some("first"), Prompt, 0);
    event(&mut store, "native", Some("first"), Stop, 60);
    let (q, _) = body(&mut store, &first, 61);
    store
        .finish(&q, "acknowledged", "ACCEPTED", clock(61).wall, None)
        .unwrap();
    let second = store.approve(binding()).unwrap();
    event(&mut store, "native", Some("second"), Prompt, 100);
    event(&mut store, "native", Some("second"), Stop, 160);
    let (_, p) = body(&mut store, &second, 161);
    assert_eq!(p["revision"], 2);
    store.control(&repo(), "disable", &clock(170)).unwrap();
    let renewed = store.approve(binding()).unwrap();
    assert_eq!(renewed.id, second.id);
}
#[cfg(unix)]
#[test]
fn absent_linked_hint_inherits_approval_but_explicit_conflict_is_held() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main");
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
    std::fs::create_dir(root.join(".keito")).unwrap();
    std::fs::write(
        root.join(".keito/config.yml"),
        "project_id: p_fixture\ntask_id: t_fixture\nagent_tracking:\n  enabled: true\n",
    )
    .unwrap();
    let linked = temp.path().join("linked");
    git(
        &root,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let main = Repository::resolve(&root).unwrap();
    let worktree = Repository::resolve(&linked).unwrap();
    let mut b = binding();
    b.repository_key = main.key.clone();
    b.remote_hash = main.remote_hash.clone();
    b.hint_hash = main.hint_hash.clone();
    let mut store = Store::open(&temp.path().join("capture")).unwrap();
    store.approve(b).unwrap();
    for (kind, s) in [(Prompt, 0), (Stop, 90)] {
        store
            .event(
                &worktree,
                capture::Signal {
                    source: "codex",
                    resumed: false,
                    source_id: "linked-session",
                    turn_id: Some("turn"),
                    version: "0.167.0",
                    event: kind,
                    clock: &clock(s),
                },
            )
            .unwrap();
    }
    assert_eq!(
        store.preview("linked-session").unwrap()[0].segments[0].duration_seconds,
        90
    );
    std::fs::create_dir(linked.join(".keito")).unwrap();
    std::fs::write(linked.join(".keito/config.yml"), "project_id: other\n").unwrap();
    let conflict = Repository::resolve(&linked).unwrap();
    store
        .event(
            &conflict,
            capture::Signal {
                source: "codex",
                resumed: false,
                source_id: "wrong-session",
                turn_id: Some("turn"),
                version: "0.167.0",
                event: Prompt,
                clock: &clock(100),
            },
        )
        .unwrap();
    assert!(store.preview("wrong-session").unwrap().is_empty());
}
#[test]
fn remote_changes_do_not_change_operational_repository_family_identity() {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init"]);
    let before = Repository::resolve(temp.path()).unwrap();
    git(
        temp.path(),
        &["remote", "add", "origin", "https://example.invalid/other"],
    );
    let after = Repository::resolve(temp.path()).unwrap();
    assert_eq!(before.key, after.key);
}

#[cfg(unix)]
#[test]
fn remapping_a_native_session_is_held_instead_of_starting_revision_one_again() {
    let (_dir, mut store, first) = setup();
    event(&mut store, "native-remap", Some("first"), Prompt, 0);
    event(&mut store, "native-remap", Some("first"), Stop, 60);
    let mut changed = binding();
    changed.project_id = "other_project".into();
    let next = store.approve(changed).unwrap();
    assert_ne!(first.id, next.id);
    assert!(store
        .event(
            &repo(),
            capture::Signal {
                source: "codex",
                resumed: false,
                source_id: "native-remap",
                turn_id: Some("second"),
                version: "fixture-v1",
                event: Prompt,
                clock: &clock(100)
            }
        )
        .is_err());
    let sessions = store.preview("native-remap").unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].revision, 1);
    assert!(sessions[0].needs_attention);
    assert_eq!(store.status().unwrap().held, 1);
    assert_eq!(
        store.status().unwrap().queue[0].reason.as_deref(),
        Some("MAPPING_CHANGED_REVIEW_REQUIRED")
    );
}
#[cfg(unix)]
#[test]
fn all_terminal_events_during_pause_reconcile_without_reopening_the_turn() {
    for terminal in [Interrupt, SessionEnd] {
        let (_dir, mut store, _) = setup();
        event(&mut store, "pause-final", Some("turn"), Prompt, 0);
        store.control(&repo(), "pause", &clock(30)).unwrap();
        event(&mut store, "pause-final", Some("turn"), terminal, 60);
        store.control(&repo(), "resume", &clock(100)).unwrap();
        event(&mut store, "pause-final", Some("turn"), Stop, 150);
        let s = &store.preview("pause-final").unwrap()[0];
        assert_eq!(
            s.segments.iter().map(|s| s.duration_seconds).sum::<i64>(),
            30
        );
        assert!(s.needs_attention);
        assert_eq!(store.status().unwrap().unfinished, 0);
    }
}

#[cfg(unix)]
#[test]
fn storage_failure_requires_explicit_recovery_and_never_reconstructs_missing_seconds() {
    let (dir, mut store, _) = setup();
    event(&mut store, "unfinished", Some("turn"), Prompt, 0);
    capture::record_failure(
        &dir.path().join("capture"),
        &capture::hash("refused-boundary"),
        "STORAGE_UNAVAILABLE",
    )
    .unwrap();
    let status = store.status().unwrap();
    assert!(status.storage_paused);
    assert_eq!(
        status.capture_failure.unwrap().reason,
        "STORAGE_UNAVAILABLE"
    );
    assert!(store
        .event(
            &repo(),
            capture::Signal {
                source: "codex",
                resumed: false,
                source_id: "new",
                turn_id: Some("new"),
                version: "fixture-v1",
                event: Prompt,
                clock: &clock(10)
            }
        )
        .is_err());
    store.recover(&clock(1000)).unwrap();
    assert!(!store.status().unwrap().storage_paused);
    let s = &store.preview("unfinished").unwrap()[0];
    assert_eq!(s.segments[0].duration_seconds, 0);
    assert_eq!(s.segments[0].completeness, "INCOMPLETE");
    assert!(s.needs_attention);
}
#[cfg(unix)]
#[test]
fn queue_ceiling_reports_paused_reason_without_evicting_existing_evidence() {
    let (dir, mut store, b) = setup();
    event(&mut store, "existing", Some("turn"), Prompt, 0);
    event(&mut store, "existing", Some("turn"), Stop, 90);
    let (q, _) = body(&mut store, &b, 91);
    let database = rusqlite::Connection::open(dir.path().join("capture/capture.sqlite3")).unwrap();
    database
        .execute("CREATE TABLE synthetic_full_queue (bytes BLOB)", [])
        .unwrap();
    database
        .execute(
            "INSERT INTO synthetic_full_queue VALUES(zeroblob(104780000))",
            [],
        )
        .unwrap();
    let status = store.status().unwrap();
    assert!(status.storage_paused);
    assert_eq!(
        status.storage_reason.as_deref(),
        Some("QUEUE_LIMIT_REACHED")
    );
    assert_eq!(status.pending, 1);
    assert!(store
        .event(
            &repo(),
            capture::Signal {
                source: "codex",
                resumed: false,
                source_id: "blocked",
                turn_id: Some("turn"),
                version: "fixture-v1",
                event: Prompt,
                clock: &clock(100)
            }
        )
        .is_err());
    let body: String = database
        .query_row("SELECT body FROM outbox WHERE id=?1", [q.id], |r| r.get(0))
        .unwrap();
    assert_eq!(body, q.body);
}

#[cfg(unix)]
#[test]
fn typed_acknowledgement_retires_only_expired_acknowledged_payloads() {
    let (_dir, mut store, b) = setup();
    for n in 0..2 {
        let id = format!("turn-{n}");
        event(&mut store, "retention", Some(&id), Prompt, n * 100);
        event(&mut store, "retention", Some(&id), Stop, n * 100 + 90);
    }
    let (q, _) = body(&mut store, &b, 300);
    store
        .acknowledge(&q, "canonical-retention", clock(300).wall - 8 * 24 * 3600)
        .unwrap();
    let (next, _) = body(&mut store, &b, 301);
    store
        .acknowledge(&next, "canonical-retention", clock(301).wall)
        .unwrap();
    // Domain revision/session state remains, but an old local transport receipt is disposable.
    let state = store.preview("retention").unwrap();
    assert_eq!(state[0].revision, 2);
    assert_eq!(
        state[0].canonical_server_id.as_deref(),
        Some("canonical-retention")
    );
    let dbpath = _dir.path().join("capture/capture.sqlite3");
    let db = rusqlite::Connection::open(dbpath).unwrap();
    let old: i64 = db
        .query_row("SELECT COUNT(*) FROM outbox WHERE id=?1", [q.id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(old, 0);
}

#[test]
fn legacy_yaml_and_json_mapping_hints_have_the_same_minimized_identity() {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init"]);
    std::fs::create_dir(temp.path().join(".keito")).unwrap();
    let file = temp.path().join(".keito/config.yml");
    std::fs::write(&file,"workspace_id: co_fixture\nproject_id: p_fixture\ntask_id: t_fixture\nagent_tracking:\n  enabled: true\n  privacy: metadata-only\nmetadata:\n  ignored: PRIVATE_CANARY\n").unwrap();
    let yaml = Repository::resolve(temp.path()).unwrap();
    std::fs::write(&file,r#"{"workspace_id":"co_fixture","project_id":"p_fixture","task_id":"t_fixture","agent_tracking":{"enabled":true,"privacy":"metadata-only"},"metadata":{"ignored":"OTHER_IGNORED_CANARY"}}"#).unwrap();
    let json = Repository::resolve(temp.path()).unwrap();
    assert_eq!(yaml.hint_hash, json.hint_hash);
}
#[test]
fn mapping_parser_refuses_deep_resource_amplification_without_echo() {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init"]);
    std::fs::create_dir(temp.path().join(".keito")).unwrap();
    let file = temp.path().join(".keito/config.yml");
    let body = format!(
        "project_id: p_fixture\nmetadata:\n  deep: {}PRIVATE_CANARY{}\n",
        "[".repeat(32),
        "]".repeat(32)
    );
    std::fs::write(&file, body).unwrap();
    let error = Repository::resolve(temp.path()).unwrap_err().to_string();
    assert!(!error.contains("PRIVATE_CANARY"));
}
#[test]
fn malformed_and_duplicate_mapping_keys_are_refused_without_source_excerpt() {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init"]);
    std::fs::create_dir(temp.path().join(".keito")).unwrap();
    let file = temp.path().join(".keito/config.yml");
    for body in [
        "project_id: p_fixture\nproject_id: another_project\n",
        "project_id: [PRIVATE_CANARY\n",
    ] {
        std::fs::write(&file, body).unwrap();
        let error = Repository::resolve(temp.path()).unwrap_err().to_string();
        assert!(!error.contains("PRIVATE_CANARY"));
    }
}

#[cfg(unix)]
#[test]
fn global_revoke_and_reenable_cannot_resurrect_an_active_turn() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "revoked-live", Some("turn"), Prompt, 0);
    store.disable_all().unwrap();
    event(&mut store, "revoked-live", Some("turn"), Stop, 60);
    store.approve(binding()).unwrap();
    event(&mut store, "revoked-live", Some("turn"), Stop, 150);
    let s = &store.preview("revoked-live").unwrap()[0];
    assert_eq!(
        s.segments.iter().map(|s| s.duration_seconds).sum::<i64>(),
        0
    );
    assert!(s.needs_attention);
    assert_eq!(store.status().unwrap().unfinished, 0);
}
#[cfg(unix)]
#[test]
fn removed_harness_and_reapproval_cannot_count_the_unapproved_interval() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "removed-live", Some("turn"), Prompt, 0);
    let mut remaining = binding();
    remaining.agents = vec!["claude-code".into()];
    store.approve(remaining).unwrap();
    event(&mut store, "removed-live", Some("turn"), Stop, 60);
    store.approve(binding()).unwrap();
    event(&mut store, "removed-live", Some("turn"), Stop, 150);
    let s = &store.preview("removed-live").unwrap()[0];
    assert_eq!(
        s.segments.iter().map(|s| s.duration_seconds).sum::<i64>(),
        0
    );
    assert!(s.needs_attention);
}
#[cfg(unix)]
#[test]
fn refused_stop_cannot_be_replaced_by_the_time_of_a_later_duplicate_stop() {
    let (dir, mut store, _) = setup();
    event(&mut store, "refused-stop", Some("turn"), Prompt, 0);
    capture::record_failure(
        &dir.path().join("capture"),
        &capture::hash("refused-stop"),
        "STORAGE_UNAVAILABLE",
    )
    .unwrap();
    event(&mut store, "refused-stop", Some("turn"), Stop, 150);
    let s = &store.preview("refused-stop").unwrap()[0];
    assert_eq!(
        s.segments.iter().map(|s| s.duration_seconds).sum::<i64>(),
        0
    );
    assert!(s.needs_attention);
    assert!(s.segments.iter().all(|s| s.completeness == "INCOMPLETE"));
    assert!(store.status().unwrap().storage_paused);
}

#[cfg(unix)]
#[test]
fn refusal_is_reconciled_before_pause_and_resume_clock_transitions() {
    let (dir, mut store, _) = setup();
    event(&mut store, "refused-control", Some("turn"), Prompt, 0);
    capture::record_failure(
        &dir.path().join("capture"),
        &capture::hash("boundary"),
        "STORAGE_UNAVAILABLE",
    )
    .unwrap();
    store.control(&repo(), "pause", &clock(150)).unwrap();
    let _ = store.control(&repo(), "resume", &clock(170));
    event(&mut store, "refused-control", Some("turn"), Stop, 180);
    let session = &store.preview("refused-control").unwrap()[0];
    assert_eq!(
        session
            .segments
            .iter()
            .map(|s| s.duration_seconds)
            .sum::<i64>(),
        0
    );
    assert!(session
        .segments
        .iter()
        .all(|s| s.completeness == "INCOMPLETE"));
    let database = rusqlite::Connection::open(dir.path().join("capture/capture.sqlite3")).unwrap();
    let clocks: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE json_type(data,'$.active.clock')='object'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(clocks, 0);
}
#[cfg(unix)]
#[test]
fn refusal_published_while_terminal_waits_for_sqlite_is_not_a_stale_authorization() {
    let (dir, mut store, _) = setup();
    event(&mut store, "racing-terminal", Some("turn"), Prompt, 0);
    let database = rusqlite::Connection::open(dir.path().join("capture/capture.sqlite3")).unwrap();
    database.execute_batch("BEGIN IMMEDIATE").unwrap();
    let worker = std::thread::spawn(move || {
        event(&mut store, "racing-terminal", Some("turn"), Stop, 150);
        store
    });
    std::thread::sleep(std::time::Duration::from_millis(100));
    capture::record_failure(
        &dir.path().join("capture"),
        &capture::hash("boundary"),
        "STORAGE_UNAVAILABLE",
    )
    .unwrap();
    database.execute_batch("ROLLBACK").unwrap();
    let store = worker.join().unwrap();
    let session = &store.preview("racing-terminal").unwrap()[0];
    assert_eq!(
        session
            .segments
            .iter()
            .map(|s| s.duration_seconds)
            .sum::<i64>(),
        0
    );
    assert!(session.needs_attention);
}
#[cfg(unix)]
#[test]
fn observed_mapping_invalidation_is_durable_until_explicit_reapproval() {
    for remote in [false, true] {
        let (_dir, mut store, binding) = setup();
        event(&mut store, "invalidated", Some("turn"), Prompt, 0);
        let mut changed = repo();
        if remote {
            changed.remote_hash = capture::hash("changed");
        } else {
            changed.disabled = true;
        }
        store
            .event(
                &changed,
                capture::Signal {
                    source: "codex",
                    resumed: false,
                    source_id: "invalidated",
                    turn_id: Some("turn"),
                    version: "fixture-v1",
                    event: Stop,
                    clock: &clock(60),
                },
            )
            .unwrap();
        event(&mut store, "invalidated", Some("turn"), Stop, 150);
        let session = &store.preview("invalidated").unwrap()[0];
        assert_eq!(
            session
                .segments
                .iter()
                .map(|s| s.duration_seconds)
                .sum::<i64>(),
            0
        );
        assert!(session.needs_attention);
        assert!(store.binding(&repo().key).unwrap().unwrap().revoked);
        assert!(store.claim(&binding, clock(160).wall).unwrap().is_none());
        store.approve(binding.clone()).unwrap();
        assert!(store.claim(&binding, clock(170).wall).unwrap().is_some());
    }
}
#[cfg(unix)]
#[test]
fn repo_disable_checkpoints_previously_earned_fragments_without_another_native_hook() {
    let (_dir, mut store, binding) = setup();
    event(&mut store, "earned", Some("turn"), Prompt, 0);
    store.control(&repo(), "pause", &clock(30)).unwrap();
    store.control(&repo(), "disable", &clock(60)).unwrap();
    assert!(store.claim(&binding, clock(65).wall).unwrap().is_none());
    store.approve(binding.clone()).unwrap();
    let (_, body) = body(&mut store, &binding, 100);
    assert_eq!(body["segments"][0]["duration_seconds"], 30);
}
#[cfg(unix)]
#[test]
fn idle_maintenance_expires_only_acknowledged_bodies_with_an_injected_clock() {
    let (dir, mut store, binding) = setup();
    event(&mut store, "expired", Some("turn"), Prompt, 0);
    event(&mut store, "expired", Some("turn"), Stop, 30);
    let (row, _) = body(&mut store, &binding, 31);
    store.acknowledge(&row, "server", clock(31).wall).unwrap();
    event(&mut store, "unsent", Some("turn"), Prompt, 40);
    event(&mut store, "unsent", Some("turn"), Stop, 70);
    store.maintain(clock(31).wall + 8 * 24 * 3600).unwrap();
    let database = rusqlite::Connection::open(dir.path().join("capture/capture.sqlite3")).unwrap();
    let acknowledged: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM outbox WHERE state='acknowledged'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(acknowledged, 0);
    assert_eq!(store.status().unwrap().pending, 1);
    assert_eq!(store.preview("expired").unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn refusal_quarantine_survives_recover_reapproval_and_an_inflight_retry_finish() {
    let (dir, mut store, binding) = setup();
    event(&mut store, "uncertain-frozen", Some("turn"), Prompt, 0);
    event(&mut store, "uncertain-frozen", Some("turn"), Stop, 30);
    let (row, _) = body(&mut store, &binding, 31);
    let original = row.body.clone();
    capture::record_failure(
        &dir.path().join("capture"),
        &capture::hash("late-refusal"),
        "STORAGE_UNAVAILABLE",
    )
    .unwrap();
    store.maintain(clock(32).wall).unwrap();
    store
        .finish(&row, "pending", "NETWORK_UNAVAILABLE", clock(33).wall, None)
        .unwrap();
    store.recover(&clock(40)).unwrap();
    store.approve(binding.clone()).unwrap();
    assert!(store.claim(&binding, clock(100).wall).unwrap().is_none());
    let status = store.status().unwrap();
    assert_eq!(status.held, 1);
    assert_eq!(
        status.queue[0].reason.as_deref(),
        Some("REFUSED_BOUNDARY_REVIEW_REQUIRED")
    );
    let database = rusqlite::Connection::open(dir.path().join("capture/capture.sqlite3")).unwrap();
    let frozen: String = database
        .query_row("SELECT body FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(frozen, original);
}
#[cfg(unix)]
#[test]
fn observing_repository_in_sync_abandons_clocks_before_identity_can_be_restored() {
    let (_dir, mut store, binding) = setup();
    event(&mut store, "sync-observed", Some("turn"), Prompt, 0);
    let mut changed = repo();
    changed.disabled = true;
    store.observe_repository(&changed).unwrap();
    store.observe_repository(&repo()).unwrap();
    event(&mut store, "sync-observed", Some("turn"), Stop, 90);
    assert_eq!(
        store.preview("sync-observed").unwrap()[0].segments[0].duration_seconds,
        0
    );
    assert!(store.claim(&binding, clock(100).wall).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn a_stop_from_an_unapproved_repository_cannot_leave_the_same_native_clock_alive() {
    let (_dir, mut store, _) = setup();
    event(&mut store, "native-gap", Some("turn"), Prompt, 0);
    let mut unapproved = repo();
    unapproved.key = capture::hash("different-unapproved-repo");
    store
        .event(
            &unapproved,
            capture::Signal {
                source: "codex",
                resumed: false,
                source_id: "native-gap",
                turn_id: Some("turn"),
                version: "fixture-v1",
                event: Stop,
                clock: &clock(60),
            },
        )
        .unwrap();
    event(&mut store, "native-gap", Some("turn"), Stop, 150);
    let session = &store.preview("native-gap").unwrap()[0];
    assert_eq!(session.segments[0].duration_seconds, 0);
    assert!(session.needs_attention);
}

#[cfg(unix)]
#[test]
fn native_resume_during_capture_pause_abandons_old_turn_for_both_harnesses() {
    for source in ["codex", "claude-code"] {
        let (_dir, mut store, binding) = setup();
        for (event, resumed, n) in [(Prompt, false, 0), (SessionStart, true, 100)] {
            if event == SessionStart {
                store.control(&repo(), "pause", &clock(30)).unwrap();
            }
            store
                .event(
                    &repo(),
                    capture::Signal {
                        source,
                        resumed,
                        source_id: "paused-native-resume",
                        turn_id: Some("native-turn"),
                        version: "fixture-v1",
                        event,
                        clock: &clock(n),
                    },
                )
                .unwrap();
        }
        let paused = store.status().unwrap();
        assert!(paused.bindings[0].paused);
        assert_eq!(
            paused.unfinished, 0,
            "native resume must remove the old continuation for {source}"
        );
        assert_eq!(
            paused.pending, 1,
            "earned fragments must be checkpointed for {source}"
        );
        store.control(&repo(), "resume", &clock(150)).unwrap();
        store
            .event(
                &repo(),
                capture::Signal {
                    source,
                    resumed: false,
                    source_id: "paused-native-resume",
                    turn_id: Some("native-turn"),
                    version: "fixture-v1",
                    event: Stop,
                    clock: &clock(200),
                },
            )
            .unwrap();
        let session = &store.preview("paused-native-resume").unwrap()[0];
        assert_eq!(
            session
                .segments
                .iter()
                .map(|v| v.duration_seconds)
                .sum::<i64>(),
            30,
            "{source}"
        );
        assert!(session.needs_attention);
        let (_, payload) = body(&mut store, &binding, 210);
        assert!(
            payload["segments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|segment| segment["completeness"] == "INCOMPLETE"
                    && segment["duration_seconds"] == 0),
            "incomplete continuation must reach canonical review for {source}"
        );
        assert_eq!(
            payload["segments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["duration_seconds"].as_i64().unwrap())
                .sum::<i64>(),
            30
        );
    }
}
