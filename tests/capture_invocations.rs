#![cfg(unix)]
#[path = "support/mutation_environment.rs"]
mod isolation;
use keito_cli::capture::{self, Binding, Clock, Repository, Signal, Store};
use keito_cli::cli::capture::CaptureEvent::*;
use std::{fs, io::Write, os::fd::AsRawFd, os::unix::fs::OpenOptionsExt, path::Path};
const PENDING: &[u8] = b"KEITO_CAPTURE_PENDING_V1\n";
fn clock(n: i64) -> Clock {
    Clock {
        wall: 1794067200 + n,
        monotonic: 1000 + n,
        boot: "fixture".into(),
    }
}
fn repo() -> Repository {
    Repository {
        key: capture::hash("r"),
        remote_hash: capture::hash("remote"),
        hint_hash: capture::hash("hint"),
        linked_without_hint: false,
        disabled: false,
    }
}
fn fixture() -> (tempfile::TempDir, Store, Binding) {
    let t = tempfile::tempdir().unwrap();
    let mut s = Store::open(&t.path().canonicalize().unwrap()).unwrap();
    let b = s
        .approve(Binding {
            repository_key: repo().key,
            remote_hash: repo().remote_hash,
            hint_hash: repo().hint_hash,
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
    (t, s, b)
}
fn marker(root: &Path, nonce: &str, live: bool) -> fs::File {
    use std::os::unix::fs::PermissionsExt;
    let root = root.join("capture-invocations");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(format!("{nonce}.pending")))
        .unwrap();
    file.write_all(PENDING).unwrap();
    file.sync_all().unwrap();
    if live {
        assert_eq!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
    }
    file
}
fn event(s: &mut Store, e: keito_cli::cli::capture::CaptureEvent, n: i64) {
    s.event(
        &repo(),
        Signal {
            source: "codex",
            resumed: false,
            source_id: "session",
            turn_id: Some("turn"),
            version: "0.167.0",
            event: e,
            clock: &clock(n),
        },
    )
    .unwrap();
}
#[test]
fn core_never_entered_marker_prevents_a_later_stop_from_inventing_complete_seconds() {
    let (t, mut s, b) = fixture();
    event(&mut s, Prompt, 0);
    let nonce = capture::hash("never-entered");
    drop(marker(&t.path().canonicalize().unwrap(), &nonce, false));
    event(&mut s, Stop, 150);
    let v = s.preview("session").unwrap();
    assert_eq!(v[0].segments[0].duration_seconds, 0);
    assert!(v[0].needs_attention);
    assert!(s.claim(&b, clock(160).wall).unwrap().is_none());
    assert!(s.status().unwrap().storage_paused);
}
#[test]
fn live_guard_blocks_delivery_and_only_own_live_nonce_allows_hook_accounting() {
    let (t, mut s, b) = fixture();
    let root = t.path().canonicalize().unwrap();
    let nonce = capture::hash("live");
    let _guard = marker(&root, &nonce, true);
    s.hook_invocation(&nonce).unwrap();
    event(&mut s, Prompt, 0);
    event(&mut s, Stop, 30);
    assert!(!s.has_ready(&b, clock(31).wall).unwrap());
    assert!(s.claim(&b, clock(31).wall).unwrap().is_none());
    assert!(s.recover(&clock(32)).is_err());
    assert_eq!(
        s.preview("session").unwrap()[0].segments[0].duration_seconds,
        30
    );
}
#[test]
fn dead_guard_cannot_be_impersonated_or_acknowledged_by_a_late_core() {
    let (t, mut s, _) = fixture();
    let nonce = capture::hash("dead");
    drop(marker(&t.path().canonicalize().unwrap(), &nonce, false));
    assert!(s.hook_invocation(&nonce).is_err());
}
#[test]
fn dead_invocation_quarantines_existing_frozen_complete_bodies_through_recovery() {
    let (t, mut s, b) = fixture();
    event(&mut s, Prompt, 0);
    event(&mut s, Stop, 30);
    let row = s.claim(&b, clock(31).wall).unwrap().unwrap();
    let nonce = capture::hash("missing-ack");
    drop(marker(&t.path().canonicalize().unwrap(), &nonce, false));
    s.maintain(clock(32).wall).unwrap();
    s.recover(&clock(40)).unwrap();
    assert!(s.claim(&b, clock(50).wall).unwrap().is_none());
    let db = rusqlite::Connection::open(t.path().join("capture.sqlite3")).unwrap();
    let frozen: String = db
        .query_row("SELECT body FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(frozen, row.body);
    assert!(!s.status().unwrap().storage_paused);
    assert_eq!(s.status().unwrap().held, 1);
}

#[test]
fn abandoned_preparing_prefix_is_reconciled_and_recoverable() {
    let (t, mut s, b) = fixture();
    event(&mut s, Prompt, 0);
    let directory = t.path().join("capture-invocations");
    fs::create_dir(&directory).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let nonce = capture::hash("preparing-kill");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join(format!("{nonce}.preparing")))
        .unwrap();
    file.write_all(&PENDING[..8]).unwrap();
    file.sync_all().unwrap();
    drop(file);
    event(&mut s, Stop, 150);
    assert_eq!(
        s.preview("session").unwrap()[0].segments[0].duration_seconds,
        0
    );
    assert!(s.claim(&b, clock(155).wall).unwrap().is_none());
    s.recover(&clock(160)).unwrap();
    assert!(!s.status().unwrap().storage_paused);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
}
#[test]
fn replaced_nonce_inode_cannot_authorize_a_late_hook() {
    let (t, mut s, _) = fixture();
    let root = t.path().canonicalize().unwrap();
    let nonce = capture::hash("replaced");
    let guard = marker(&root, &nonce, true);
    s.hook_invocation(&nonce).unwrap();
    drop(guard);
    fs::remove_file(root.join(format!("capture-invocations/{nonce}.pending"))).unwrap();
    let _replacement = marker(&root, &nonce, true);
    assert!(s
        .event(
            &repo(),
            Signal {
                source: "codex",
                resumed: false,
                source_id: "session",
                turn_id: Some("turn"),
                version: "0.167.0",
                event: Prompt,
                clock: &clock(0)
            }
        )
        .is_err());
    assert!(s.preview("session").unwrap().is_empty());
}
#[test]
fn malformed_marker_and_symlink_are_never_healthy_or_read_as_an_invocation() {
    let (t, s, _) = fixture();
    let root = t.path().canonicalize().unwrap();
    let nonce = capture::hash("invalid");
    drop(marker(&root, &nonce, false));
    let path = root.join(format!("capture-invocations/{nonce}.pending"));
    fs::write(&path, b"PRIVATE_MALFORMED_CANARY").unwrap();
    assert!(s.status().is_err());
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(root.join("capture.sqlite3"), &path).unwrap();
    assert!(s.status().is_err());
}

#[test]
fn unqualified_actual_hook_never_acknowledges_or_accepts_a_guarded_boundary() {
    use std::{
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
    };
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir(&home).unwrap();
    let directory = home.join("capture");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let core = base.join("keito");
    fs::copy(assert_cmd::cargo::cargo_bin!("keito"), &core).unwrap();
    let checked =
        isolation::checked(&base, &base, Some(&home), Some(&directory), Some(&core)).unwrap();
    let mut store = Store::open(&directory).unwrap();
    let binding = Binding {
        repository_key: repo().key,
        remote_hash: repo().remote_hash,
        hint_hash: repo().hint_hash,
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
    };
    store.approve(binding).unwrap();
    for malformed in [false, true] {
        let nonce = capture::hash(if malformed {
            "bad-input"
        } else {
            "valid-input"
        });
        let guard = marker(&directory, &nonce, true);
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let mut child = Command::new(&checked.core)
            .env_clear()
            .current_dir(&home)
            .env("HOME", &checked.home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("KEITO_CAPTURE_HOME", &checked.capture)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("APPDATA", home.join("AppData"))
            .env("PATH", base.join("empty-bin"))
            .env("KEITO_CAPTURE_INVOCATION_NONCE", &nonce)
            .env("KEITO_CAPTURE_ACK_FD", pipe[1].to_string())
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
        let input = if malformed {
            b"{PRIVATE_RAW_CANARY".as_slice()
        } else {
            b"{\"session_id\":\"native\",\"turn_id\":\"turn\",\"prompt\":\"PRIVATE_RAW_CANARY\"}"
                .as_slice()
        };
        let _ = child.stdin.take().unwrap().write_all(input);
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_RAW_CANARY"));
        unsafe {
            libc::close(pipe[1]);
            let flags = libc::fcntl(pipe[0], libc::F_GETFL);
            libc::fcntl(pipe[0], libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let mut bytes = [0; 128];
        assert!(unsafe { libc::read(pipe[0], bytes.as_mut_ptr().cast(), bytes.len()) } <= 0);
        unsafe {
            libc::close(pipe[0]);
        }
        assert_eq!(store.status().unwrap().unfinished, 0);
        assert!(directory
            .join(format!("capture-invocations/{nonce}.pending"))
            .exists());
        drop(guard);
    }
    store.maintain(clock(100).wall).unwrap();
    assert!(store.status().unwrap().storage_paused);
}
