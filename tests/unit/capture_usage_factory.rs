//! Negative normal-factory admission. No synthetic positive approval or actual
//! process/harness/provider is constructed by this suite.
use super::*;
use crate::capture_usage_consent::ConsentPolicy;
use crate::capture_usage_test_io::Denied;

const SESSION: &str = "12345678-1234-4234-8234-123456789abc";
struct Fixture {
    _temp: tempfile::TempDir,
    store: Store,
    repository: Repository,
    binding: Binding,
    source: &'static str,
    version: &'static str,
}
impl Fixture {
    fn new(source: &'static str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("capture")).unwrap();
        let repository = Repository {
            key: hash("repo"),
            remote_hash: hash("remote"),
            hint_hash: hash("hint"),
            linked_without_hint: false,
            disabled: false,
        };
        let binding = store
            .approve(Binding {
                repository_key: repository.key.clone(),
                remote_hash: repository.remote_hash.clone(),
                hint_hash: repository.hint_hash.clone(),
                id: String::new(),
                version: 0,
                workspace_id: "co_synthetic".into(),
                owner_id: "owner_synthetic".into(),
                origin_hash: hash("origin"),
                project_id: "project_synthetic".into(),
                task_id: "task_synthetic".into(),
                agents: vec![source.into()],
                paused: false,
                revoked: false,
            })
            .unwrap();
        Self {
            _temp: temp,
            store,
            repository,
            binding,
            source,
            version: if source == "codex" {
                "0.167.0"
            } else {
                "2.1.319"
            },
        }
    }
    fn consent(&self) {
        let policy = ConsentPolicy {
            policy_version: 1,
            binding_id: self.binding.id.clone(),
            binding_version: self.binding.version,
            source_namespace: self.source.into(),
            profile_id: "a".repeat(64),
            profile_digest: "b".repeat(64),
            epoch: 1,
            enabled: true,
            unavailable_reason: None,
        };
        self.store
            .connection
            .execute(
                "INSERT INTO usage_consents(binding_id,namespace,data) VALUES(?1,?2,?3)",
                params![
                    self.binding.id,
                    self.source,
                    serde_json::to_string(&policy).unwrap()
                ],
            )
            .unwrap();
    }
    fn event(&mut self, event: CaptureEvent, n: i64, hook: bool) {
        let clock = Clock {
            wall: 1_794_067_200 + n,
            monotonic: 1000 + n,
            boot: "synthetic".into(),
        };
        let signal = Signal {
            source: self.source,
            resumed: false,
            source_id: SESSION,
            turn_id: Some("turn_synthetic"),
            version: self.version,
            event,
            clock: &clock,
        };
        if hook {
            self.store
                .event_hook(
                    &self.repository,
                    signal,
                    Some(Path::new("/inert/never-open-source")),
                    None,
                )
                .unwrap();
        } else {
            self.store.event(&self.repository, signal).unwrap();
        }
    }
    fn assert_time_only(&self) {
        let body: String = self
            .store
            .connection
            .query_row("SELECT body FROM outbox", [], |row| row.get(0))
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["segments"][0]["duration_seconds"], 4);
        assert_eq!(body["usage"]["status"], "UNAVAILABLE");
        assert!(body["usage"]["records"]
            .as_array()
            .is_none_or(Vec::is_empty));
        assert!(!body.to_string().contains("/inert/"));
    }
}
#[test]
fn default_and_explicitly_disabled_never_consult_numeric_io() {
    for source in ["codex", "claude-code"] {
        for disabled in [false, true] {
            let mut f = Fixture::new(source);
            if disabled {
                f.store.disable_numeric(&f.repository, source).unwrap();
            }
            let denied = Denied::arm();
            f.event(CaptureEvent::Prompt, 0, true);
            f.event(CaptureEvent::Stop, 4, true);
            assert_eq!(denied.counts(), [0; 3]);
            f.assert_time_only();
        }
    }
}
#[test]
fn enabled_without_live_guard_keeps_timing_and_never_loads_profile_or_source() {
    for source in ["codex", "claude-code"] {
        let mut f = Fixture::new(source);
        f.consent();
        let denied = Denied::arm();
        f.event(CaptureEvent::Prompt, 0, true);
        assert_eq!(
            f.store.numeric_status(&f.repository, source).unwrap()["reason"],
            "LIVE_GUARD_REQUIRED"
        );
        f.event(CaptureEvent::Stop, 4, true);
        assert_eq!(
            f.store.numeric_status(&f.repository, source).unwrap()["reason"],
            "MISSING_QUALIFIED_PROMPT"
        );
        assert_eq!(denied.counts(), [0; 3]);
        f.assert_time_only();
    }
}
#[test]
fn terminal_cannot_repair_an_unqualified_prompt() {
    for source in ["codex", "claude-code"] {
        let mut f = Fixture::new(source);
        f.event(CaptureEvent::Prompt, 0, false);
        f.consent();
        let denied = Denied::arm();
        f.event(CaptureEvent::Stop, 4, true);
        assert_eq!(
            f.store.numeric_status(&f.repository, source).unwrap()["reason"],
            "MISSING_QUALIFIED_PROMPT"
        );
        assert_eq!(denied.counts(), [0; 3]);
        f.assert_time_only();
    }
}
#[test]
fn committed_revoke_before_hook_wins_without_any_functional_read() {
    for source in ["codex", "claude-code"] {
        let mut f = Fixture::new(source);
        f.consent();
        f.store.disable_numeric(&f.repository, source).unwrap();
        let denied = Denied::arm();
        f.event(CaptureEvent::Prompt, 0, true);
        f.event(CaptureEvent::Stop, 4, true);
        assert_eq!(
            f.store.numeric_status(&f.repository, source).unwrap()["enabled"],
            false
        );
        assert_eq!(denied.counts(), [0; 3]);
        f.assert_time_only();
    }
}
#[test]
fn malformed_or_binding_stale_consent_cannot_trigger_functional_reads() {
    for source in ["codex", "claude-code"] {
        for malformed in [false, true] {
            let mut f = Fixture::new(source);
            f.consent();
            f.store
                .connection
                .execute(
                    "UPDATE usage_consents SET data=?1",
                    [if malformed {
                        "{malformed".to_owned()
                    } else {
                        let mut policy =
                            Store::numeric_policy_in(&f.store.connection, &f.binding, source)
                                .unwrap()
                                .unwrap();
                        policy.binding_version += 1;
                        serde_json::to_string(&policy).unwrap()
                    }],
                )
                .unwrap();
            let denied = Denied::arm();
            f.event(CaptureEvent::Prompt, 0, true);
            f.event(CaptureEvent::Stop, 4, true);
            assert_eq!(denied.counts(), [0; 3]);
            f.assert_time_only();
        }
    }
}

fn record(id: &str) -> crate::capture_usage_ledger::UsageRecord {
    crate::capture_usage_ledger::UsageRecord {
        source_adapter: "CLAUDE_JSONL_2_1_319_V1".into(),
        source_record_id: id.into(),
        source_turn_id: None,
        model: None,
        model_provenance: "UNKNOWN".into(),
        counter_semantics: "REQUEST_DELTA".into(),
        input_basis: "CLAUDE_UNCACHED_INPUT".into(),
        input_tokens: Some(5),
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        output_tokens: Some(2),
        reasoning_output_tokens: None,
    }
}
fn frozen_fixture() -> (Fixture, Session, String, String) {
    let mut f = Fixture::new("claude-code");
    f.consent();
    f.event(CaptureEvent::Prompt, 0, false);
    f.event(CaptureEvent::Stop, 4, false);
    let raw: String = f
        .store
        .connection
        .query_row("SELECT data FROM sessions", [], |row| row.get(0))
        .unwrap();
    let mut session: Session = serde_json::from_str(&raw).unwrap();
    session.usage_ledger.records.push(record("request_frozen"));
    session.usage_ledger.record_revisions.push(1);
    session.usage_ledger.queued = 1;
    let mut body = session.payload(session.segments.clone(), 1).unwrap();
    body.usage = session.usage_ledger.payload(0, 1);
    let frozen = serde_json::to_string(&body).unwrap();
    let digest = hash(&frozen);
    f.store
        .connection
        .execute(
            "UPDATE outbox SET body=?1,payload_hash=?2",
            params![frozen, digest],
        )
        .unwrap();
    f.store
        .connection
        .execute(
            "UPDATE sessions SET data=?1",
            [serde_json::to_string(&session).unwrap()],
        )
        .unwrap();
    let start = Clock {
        wall: 1_794_067_206,
        monotonic: 1006,
        boot: "synthetic".into(),
    };
    session.active = Some(ActiveTurn {
        key: "turn_second".into(),
        clock: Some(start.clone()),
        part: 0,
    });
    let end = Clock {
        wall: start.wall + 4,
        monotonic: start.monotonic + 4,
        boot: start.boot,
    };
    session.close(&end, true).unwrap();
    (f, session, frozen, digest)
}
#[test]
fn late_source_failure_rolls_back_new_numeric_writes_but_preserves_time_and_frozen_evidence() {
    // Component-only negative: no runtime/normal approval constructor is added.
    // Genuine guarded normal-route acceptance is the separately retained pair QA.
    use crate::capture_usage_test_io::AfterWrite;
    for replace in [false, true] {
        let (mut f, time_only, frozen, digest) = frozen_fixture();
        let home = f._temp.path().canonicalize().unwrap().join("home");
        let path = home.join(format!(".claude/projects/owned/{SESSION}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        let approved =
            ApprovedActiveSession::new(Harness::ClaudeCode, "2.1.319", &home, &path, SESSION)
                .unwrap();
        let (_, lease) = crate::capture_usage::read_request_ids_leased(&approved).unwrap();
        let policy = Store::numeric_policy_in(&f.store.connection, &f.binding, f.source)
            .unwrap()
            .unwrap();
        let mut factory = NormalFactory {
            policy: Some(policy),
            runtime: None,
            approved: None,
            profile: None,
            origin: None,
            source_lease: Some(lease),
            reason: Some("OBSERVED_REQUEST_RECORDS"),
            invalidate: false,
        };
        let mut candidate = time_only.clone();
        candidate
            .usage_ledger
            .records
            .push(record("request_uncertain"));
        candidate.usage_ledger.record_revisions.push(0);
        let observed = std::rc::Rc::new(std::cell::Cell::new(false));
        let callback_observed = observed.clone();
        let _fault = AfterWrite::arm(move |connection| {
            let written: String = connection
                .query_row("SELECT body FROM outbox WHERE revision=2", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let written: serde_json::Value = serde_json::from_str(&written).unwrap();
            assert_eq!(written["usage"]["status"], "PARTIAL");
            assert_eq!(
                written["usage"]["records"][0]["source_record_id"],
                "request_uncertain"
            );
            callback_observed.set(true);
            if replace {
                fs::rename(&path, path.with_extension("previous")).unwrap();
            }
            fs::write(&path, b"PRIVATE_SOURCE_CHANGED_AFTER_QUEUE").unwrap();
        });
        let tx = f.store.transaction().unwrap();
        Store::finalize_normal_event(
            &tx,
            &mut candidate,
            Some(&mut factory),
            EventFinalization {
                id: &time_only.id,
                binding: &f.binding,
                source_id: SESSION,
                was_refused: false,
                time_only: Some(&time_only),
            },
        )
        .unwrap();
        tx.commit().unwrap();
        assert!(observed.get());
        let prior: (String, String) = f
            .store
            .connection
            .query_row(
                "SELECT body,payload_hash FROM outbox WHERE revision=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(prior, (frozen, digest));
        let new: String = f
            .store
            .connection
            .query_row("SELECT body FROM outbox WHERE revision=2", [], |row| {
                row.get(0)
            })
            .unwrap();
        let new: serde_json::Value = serde_json::from_str(&new).unwrap();
        assert_eq!(new["segments"][0]["duration_seconds"], 4);
        assert_eq!(new["usage"]["status"], "UNAVAILABLE");
        let persisted: String = f
            .store
            .connection
            .query_row("SELECT data FROM sessions", [], |row| row.get(0))
            .unwrap();
        let persisted: Session = serde_json::from_str(&persisted).unwrap();
        assert_eq!(persisted.usage_ledger.records.len(), 1);
        assert_eq!(
            persisted.usage_ledger.records[0].source_record_id,
            "request_frozen"
        );
        assert!(!persisted.usage_ledger.baseline_ready());
        assert_eq!(
            f.store.numeric_status(&f.repository, f.source).unwrap()["reason"],
            "SOURCE_CHANGED"
        );
    }
}
#[test]
fn self_revocation_keeps_specific_local_reason_and_never_rewrites_a_frozen_numeric_body() {
    let (mut f, time_only, frozen, digest) = frozen_fixture();
    let policy = Store::numeric_policy_in(&f.store.connection, &f.binding, f.source)
        .unwrap()
        .unwrap();
    let mut factory = NormalFactory {
        policy: Some(policy),
        runtime: None,
        approved: None,
        profile: None,
        origin: None,
        source_lease: None,
        reason: Some("HARNESS_CHANGED"),
        invalidate: true,
    };
    let mut candidate = time_only.clone();
    factory.fail(&mut candidate, "HARNESS_CHANGED");
    let tx = f.store.transaction().unwrap();
    Store::finalize_normal_event(
        &tx,
        &mut candidate,
        Some(&mut factory),
        EventFinalization {
            id: &time_only.id,
            binding: &f.binding,
            source_id: SESSION,
            was_refused: false,
            time_only: Some(&time_only),
        },
    )
    .unwrap();
    tx.commit().unwrap();
    let prior: (String, String, String) = f
        .store
        .connection
        .query_row(
            "SELECT body,payload_hash,state FROM outbox WHERE revision=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(prior, (frozen, digest, "held".into()));
    let status = f.store.numeric_status(&f.repository, f.source).unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["reason"], "HARNESS_CHANGED");
}
