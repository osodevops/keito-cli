//! Actual SQLite connection/callback wiring; no harness, service or CLI process.
#![cfg(unix)]
#[path = "support/mutation_environment.rs"]
mod isolation;
use keito_cli::{
    capture::{self, Binding, Clock, Repository, Signal, Store},
    cli::capture::CaptureEvent,
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    time::{Duration, Instant},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    capture: PathBuf,
    repository: Repository,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let capture = home.join("capture");
        for path in [
            &home,
            &capture,
            &home.join(".codex"),
            &home.join(".claude"),
            &home.join("config"),
            &home.join("state"),
            &root.join("empty-bin"),
            &root.join("tmp"),
        ] {
            fs::create_dir_all(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            let metadata = fs::symlink_metadata(path).unwrap();
            assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
            assert!(!metadata.file_type().is_symlink());
            assert_eq!(path.canonicalize().unwrap(), *path);
        }
        let core = root.join("inert-never-executed-core");
        fs::write(&core, b"inert test boundary only; never executable").unwrap();
        let marker = root.join("invalid-mutation");
        for missing in [0, 1, 2] {
            let result = isolation::checked(
                &root,
                &root,
                (missing != 0).then_some(home.as_path()),
                (missing != 1).then_some(capture.as_path()),
                (missing != 2).then_some(core.as_path()),
            );
            if result.is_ok() {
                fs::write(&marker, b"unsafe").unwrap();
            }
            assert!(!marker.exists());
        }
        let checked =
            isolation::checked(&root, &root, Some(&home), Some(&capture), Some(&core)).unwrap();
        assert_eq!(checked.home, home);
        assert_eq!(checked.capture, capture);
        assert_eq!(checked.core, core);
        let repository = Repository {
            key: capture::hash("wiring-repo"),
            remote_hash: capture::hash("wiring-remote"),
            hint_hash: capture::hash("wiring-hint"),
            linked_without_hint: false,
            disabled: false,
        };
        Self {
            _temp: temp,
            root,
            capture,
            repository,
        }
    }
    fn database(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.capture.join("capture.sqlite3")).unwrap()
    }
    fn store(&self) -> Store {
        Store::open(&self.capture).unwrap()
    }
    fn approve(&self, store: &mut Store) {
        store
            .approve(Binding {
                repository_key: self.repository.key.clone(),
                remote_hash: self.repository.remote_hash.clone(),
                hint_hash: self.repository.hint_hash.clone(),
                id: String::new(),
                version: 0,
                workspace_id: "workspace_wiring".into(),
                owner_id: "owner_wiring".into(),
                origin_hash: capture::hash("https://example.invalid"),
                project_id: "project_wiring".into(),
                task_id: "task_wiring".into(),
                agents: vec!["codex".into()],
                paused: false,
                revoked: false,
            })
            .unwrap();
    }
    fn event(
        &self,
        store: &mut Store,
        session: &str,
        turn: &str,
        event: CaptureEvent,
        n: i64,
    ) -> Result<(), keito_cli::error::AppError> {
        store.event(
            &self.repository,
            Signal {
                source: "codex",
                resumed: false,
                source_id: session,
                turn_id: Some(turn),
                version: "0.167.0",
                event,
                clock: &Clock {
                    wall: 1_794_067_200 + n,
                    monotonic: 1000 + n,
                    boot: "owned-wiring-fixture".into(),
                },
            },
        )
    }
    fn refused_event(&self, store: &mut Store, session: &str, turn: &str, n: i64) {
        let started = Instant::now();
        assert!(self
            .event(store, session, turn, CaptureEvent::Prompt, n)
            .is_err());
        let elapsed = started.elapsed();
        println!("actual SQLite rejected locking operation: {elapsed:?}");
        // This lower-bound control distinguishes a fresh real callback budget
        // from incorrectly retained exhausted state. The existing dedicated
        // contention case retains the unchanged 700ms upper acceptance gate.
        assert!(
            elapsed >= Duration::from_millis(450),
            "new locking event inherited an exhausted/immediate refusal"
        );
    }
}

#[test]
fn same_store_failed_lock_then_success_then_second_contention_gets_a_fresh_budget() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.approve(&mut store);
    let blocker = fixture.database();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    fixture.refused_event(&mut store, "session_same", "first", 0);
    blocker.execute_batch("ROLLBACK").unwrap();
    fixture
        .event(
            &mut store,
            "session_same",
            "first",
            CaptureEvent::Prompt,
            10,
        )
        .unwrap();
    fixture
        .event(&mut store, "session_same", "first", CaptureEvent::Stop, 40)
        .unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    fixture.refused_event(&mut store, "session_same", "second", 50);
    blocker.execute_batch("ROLLBACK").unwrap();
    fixture
        .event(
            &mut store,
            "session_same",
            "second",
            CaptureEvent::Prompt,
            60,
        )
        .unwrap();
    fixture
        .event(&mut store, "session_same", "second", CaptureEvent::Stop, 90)
        .unwrap();
    let sessions = store.preview("session_same").unwrap();
    assert_eq!(
        sessions[0]
            .segments
            .iter()
            .map(|s| s.duration_seconds)
            .collect::<Vec<_>>(),
        [30, 30]
    );
    assert!(!store.status().unwrap().storage_paused);
}

#[test]
fn independent_stores_keep_their_own_failed_and_recovered_locking_state() {
    let fixture = Fixture::new();
    let mut first = fixture.store();
    fixture.approve(&mut first);
    let mut second = fixture.store();
    let blocker = fixture.database();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    fixture.refused_event(&mut first, "session_one", "turn", 0);
    fixture.refused_event(&mut second, "session_two", "turn", 0);
    blocker.execute_batch("ROLLBACK").unwrap();
    for (store, session) in [(&mut first, "session_one"), (&mut second, "session_two")] {
        fixture
            .event(store, session, "turn", CaptureEvent::Prompt, 10)
            .unwrap();
        fixture
            .event(store, session, "turn", CaptureEvent::Stop, 40)
            .unwrap();
        assert_eq!(
            store.preview(session).unwrap()[0].segments[0].duration_seconds,
            30
        );
    }
}

#[test]
fn schema_open_installs_the_busy_budget_before_locked_schema_work() {
    let fixture = Fixture::new();
    drop(fixture.store());
    let blocker = fixture.database();
    blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let started = Instant::now();
    assert!(Store::open(&fixture.capture).is_err());
    let elapsed = started.elapsed();
    println!("locked schema/open: {elapsed:?}");
    assert!(elapsed >= Duration::from_millis(450));
    blocker.execute_batch("ROLLBACK").unwrap();
    let mut recovered = fixture.store();
    fixture.approve(&mut recovered);
    fixture
        .event(
            &mut recovered,
            "session_schema",
            "turn",
            CaptureEvent::Prompt,
            0,
        )
        .unwrap();
    fixture
        .event(
            &mut recovered,
            "session_schema",
            "turn",
            CaptureEvent::Stop,
            30,
        )
        .unwrap();
    assert_eq!(
        recovered.preview("session_schema").unwrap()[0].segments[0].duration_seconds,
        30
    );
}

#[test]
fn reader_blocked_commit_rolls_back_and_the_same_store_can_later_commit() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.approve(&mut store);
    let reader = fixture.database();
    reader
        .execute_batch("BEGIN; SELECT COUNT(*) FROM bindings")
        .unwrap();
    fixture.refused_event(&mut store, "session_commit", "turn", 0);
    #[cfg(feature = "dev-tools")]
    {
        let timing = store.last_admission_timing().unwrap();
        assert_eq!(
            timing.sqlite_error, None,
            "BEGIN must succeed before this commit-only contention"
        );
        assert!(timing.authority.is_some());
    }
    let count: i64 = reader
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "failed commit must not leave a partial session");
    reader.execute_batch("ROLLBACK").unwrap();
    fixture
        .event(
            &mut store,
            "session_commit",
            "turn",
            CaptureEvent::Prompt,
            10,
        )
        .unwrap();
    fixture
        .event(&mut store, "session_commit", "turn", CaptureEvent::Stop, 40)
        .unwrap();
    assert_eq!(
        store.preview("session_commit").unwrap()[0].segments[0].duration_seconds,
        30
    );
}

#[test]
fn uncontended_store_preserves_normal_schema_event_and_commit_behavior() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.approve(&mut store);
    fixture
        .event(
            &mut store,
            "session_control",
            "turn",
            CaptureEvent::Prompt,
            0,
        )
        .unwrap();
    fixture
        .event(
            &mut store,
            "session_control",
            "turn",
            CaptureEvent::Stop,
            30,
        )
        .unwrap();
    let sessions = store.preview("session_control").unwrap();
    assert_eq!(sessions[0].segments[0].duration_seconds, 30);
    assert_eq!(sessions[0].segments[0].completeness, "COMPLETE");
    assert!(!store.status().unwrap().storage_paused);
    assert!(!fixture.root.join("invalid-mutation").exists());
}
