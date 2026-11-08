#![cfg(unix)]
// Native consent/store/reader assertions require the supported POSIX authority.
//! Synthetic core qualification only. Native command integration remains disabled.
use keito_cli::capture::{self, Binding, Clock, Repository, Signal, Store, UsageSource};
use keito_cli::cli::capture::CaptureEvent::{self, *};
use serde_json::{json, Value};
use std::fs;
const SESSION: &str = "12345678-1234-4234-8234-123456789abc";
fn clock(n: i64) -> Clock {
    Clock {
        wall: 1_794_067_200 + n,
        monotonic: 1000 + n,
        boot: "fixture".into(),
    }
}
fn repo() -> Repository {
    Repository {
        key: capture::hash("repo"),
        remote_hash: capture::hash("remote"),
        hint_hash: capture::hash("hint"),
        linked_without_hint: false,
        disabled: false,
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    home: std::path::PathBuf,
    path: std::path::PathBuf,
    store: Store,
    binding: Binding,
    namespace: &'static str,
    version: &'static str,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        let path = home.join(format!(".claude/projects/fixture/{SESSION}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut store = Store::open(&dir.path().join("capture")).unwrap();
        let binding = store
            .approve(Binding {
                repository_key: repo().key,
                remote_hash: repo().remote_hash,
                hint_hash: repo().hint_hash,
                id: String::new(),
                version: 0,
                workspace_id: "co_fixture".into(),
                owner_id: "user_fixture".into(),
                origin_hash: capture::hash("origin"),
                project_id: "project_fixture".into(),
                task_id: "task_fixture".into(),
                agents: vec!["claude-code".into()],
                paused: false,
                revoked: false,
            })
            .unwrap();
        Self {
            dir,
            home,
            path,
            store,
            binding,
            namespace: "claude-code",
            version: "2.1.319",
        }
    }
    fn write(&self, records: Vec<Value>) {
        fs::write(
            &self.path,
            records
                .into_iter()
                .map(|v| v.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
    }
    fn event(&mut self, event: CaptureEvent, n: i64, usage: bool) {
        self.event_turn(event, n, usage, "prompt_1");
    }
    fn event_turn(&mut self, event: CaptureEvent, n: i64, usage: bool, turn: &str) {
        let source = UsageSource {
            resolved_home: &self.home,
            exact_hook_path: &self.path,
            qualified_source: self.namespace,
            qualified_native_session: SESSION,
        };
        self.store
            .event_with_usage(
                &repo(),
                Signal {
                    source: self.namespace,
                    resumed: false,
                    source_id: SESSION,
                    turn_id: Some(turn),
                    version: self.version,
                    event,
                    clock: &clock(n),
                },
                if usage { Some(&source) } else { None },
            )
            .unwrap();
    }
    fn start(&mut self, n: i64) {
        self.write(vec![]);
        self.event(Prompt, n, true);
    }
    fn usage_ids(&self) -> Vec<String> {
        self.bodies()
            .into_iter()
            .flat_map(|body| {
                body["usage"]["records"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .map(|record| record["source_record_id"].as_str().unwrap().to_owned())
            .collect()
    }
    fn bodies(&self) -> Vec<Value> {
        let db =
            rusqlite::Connection::open(self.dir.path().join("capture/capture.sqlite3")).unwrap();
        let mut s = db
            .prepare("SELECT body FROM outbox ORDER BY revision")
            .unwrap();
        s.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|s| serde_json::from_str(&s.unwrap()).unwrap())
            .collect()
    }
}
fn request(id: &str, output: u64) -> Value {
    json!({"type":"assistant","sessionId":SESSION,"message":{"id":id,"model":"claude-opus-5-5","stop_reason":"end_turn","usage":{"input_tokens":2,"cache_read_input_tokens":null,"cache_creation_input_tokens":2280,"output_tokens":output},"content":[{"text":"PRIVATE_CANARY sk-secret /private/customer"}]}})
}
#[test]
fn exact_session_usage_and_time_commit_atomically_without_prose_or_paths() {
    let mut f = Fixture::new();
    f.start(0);
    f.write(vec![request("msg_1", 5)]);
    f.event(Stop, 59, true);
    let b = &f.bodies()[0];
    assert_eq!(b["segments"][0]["duration_seconds"], 59);
    assert_eq!(b["usage"]["status"], "PARTIAL");
    assert_eq!(b["usage"]["records"][0]["cached_input_tokens"], Value::Null);
    assert_eq!(b["usage"]["records"][0]["cache_write_input_tokens"], 2280);
    let db = fs::read(f.dir.path().join("capture/capture.sqlite3")).unwrap();
    let text = String::from_utf8_lossy(&db);
    assert!(!text.contains("PRIVATE_CANARY"));
    assert!(!text.contains("sk-secret"));
    assert!(!text.contains(f.path.to_str().unwrap()));
    assert!(f.store.claim(&f.binding, clock(60).wall).unwrap().is_some());
}
#[test]
fn late_usage_has_its_own_revision_and_replay_changes_no_frozen_bytes() {
    let mut f = Fixture::new();
    f.start(0);
    f.event(Stop, 1, false);
    let first = f.bodies();
    f.write(vec![request("msg_1", 5)]);
    f.event(Stop, 10, true);
    f.event(Stop, 11, true);
    let b = f.bodies();
    assert_eq!(b.len(), 2);
    assert_eq!(b[0], first[0]);
    assert_eq!(b[1]["segments"], json!([]));
    assert_eq!(b[1]["revision"], 2);
    assert_eq!(f.store.preview(SESSION).unwrap()[0].segments.len(), 1);
}
#[test]
fn more_than_25_requests_are_disjoint_bounded_batches_without_truncation() {
    let mut f = Fixture::new();
    f.start(0);
    f.write(
        (0..80)
            .map(|n| request(&format!("msg_{n:03}_{}", "a".repeat(110)), 5))
            .collect(),
    );
    f.event(Stop, 2, true);
    let b = f.bodies();
    assert!(b.len() > 3);
    let mut ids = std::collections::BTreeSet::new();
    let mut seconds = 0;
    for body in b {
        assert!(body.to_string().len() <= capture::MAX_BODY_BYTES);
        let r = body["usage"]["records"].as_array().unwrap();
        assert!(r.len() <= 25);
        for row in r {
            assert!(ids.insert(row["source_record_id"].as_str().unwrap().to_owned()));
        }
        seconds += body["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["duration_seconds"].as_i64().unwrap())
            .sum::<i64>();
    }
    assert_eq!(ids.len(), 80);
    assert_eq!(seconds, 2);
}
#[test]
fn conflicting_request_identity_holds_unsent_evidence_but_preserves_time() {
    let mut f = Fixture::new();
    f.start(0);
    f.write(vec![request("msg_1", 5)]);
    f.event(Stop, 2, true);
    let frozen = f.bodies();
    f.write(vec![request("msg_1", 6)]);
    f.event(Stop, 3, true);
    assert!(f.store.claim(&f.binding, clock(5).wall).unwrap().is_none());
    assert!(f.store.preview(SESSION).unwrap()[0].needs_attention);
    assert_eq!(f.bodies(), frozen);
    assert_eq!(
        f.store.preview(SESSION).unwrap()[0].segments[0].duration_seconds,
        2
    );
    assert!(f
        .store
        .status()
        .unwrap()
        .queue
        .iter()
        .any(|q| q.reason.as_deref() == Some("USAGE_RECORD_CONFLICT")));
}
#[test]
fn conflict_within_one_snapshot_is_held_instead_of_omitting_conflicted_id() {
    let mut f = Fixture::new();
    f.start(0);
    f.write(vec![
        request("msg_1", 1),
        request("msg_1", 5),
        request("msg_2", 3),
    ]);
    f.event(Stop, 2, true);
    assert!(f.store.claim(&f.binding, clock(3).wall).unwrap().is_none());
    assert!(f.store.preview(SESSION).unwrap()[0].needs_attention);
    assert_eq!(
        f.store.preview(SESSION).unwrap()[0].segments[0].duration_seconds,
        2
    );
}
#[test]
fn revoked_binding_never_reads_or_accepts_transcript_usage() {
    let mut f = Fixture::new();
    f.start(0);
    f.store.control(&repo(), "disable", &clock(1)).unwrap();
    f.write(vec![request("msg_1", 5)]);
    f.event(Stop, 2, true);
    assert!(f
        .bodies()
        .iter()
        .all(|b| b["usage"]["status"] == "UNAVAILABLE"));
}
#[test]
fn missing_unsafe_and_wrong_identity_sources_never_replace_prior_usage() {
    let mut f = Fixture::new();
    f.start(0);
    f.write(vec![request("msg_1", 5)]);
    f.event(Stop, 2, true);
    let prior = f.bodies();
    fs::write(
        &f.path,
        "{\"type\":\"assistant\",\"sessionId\":\"other\"}\n",
    )
    .unwrap();
    f.event(Stop, 3, true);
    assert_eq!(f.bodies(), prior);
    let s = UsageSource {
        resolved_home: &f.home,
        exact_hook_path: &f.path,
        qualified_source: "claude-code",
        qualified_native_session: "different-session",
    };
    f.store
        .event_with_usage(
            &repo(),
            Signal {
                source: "claude-code",
                resumed: false,
                source_id: SESSION,
                turn_id: Some("prompt_1"),
                version: "2.1.319",
                event: Stop,
                clock: &clock(4),
            },
            Some(&s),
        )
        .unwrap();
    assert_eq!(f.bodies(), prior);
}
#[test]
fn legacy_metadata_capture_never_reads_transcripts_without_explicit_internal_gate() {
    let mut f = Fixture::new();
    f.write(vec![request("msg_1", 5)]);
    f.event(Prompt, 0, false);
    f.event(Stop, 2, false);
    assert_eq!(f.bodies()[0]["usage"]["status"], "UNAVAILABLE");
}

#[test]
fn provisional_request_does_not_permanently_hold_a_later_final_usage_revision() {
    let mut f = Fixture::new();
    f.start(0);
    let mut provisional = request("msg_later_final", 0);
    provisional["message"]["stop_reason"] = Value::Null;
    f.write(vec![provisional]);
    f.event(Stop, 2, true);
    let initial = f.bodies();
    assert_eq!(initial[0]["usage"]["status"], "UNAVAILABLE");
    assert!(!f.store.preview(SESSION).unwrap()[0].needs_attention);
    f.write(vec![request("msg_later_final", 5)]);
    f.event(Stop, 4, true);
    let bodies = f.bodies();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0], initial[0]);
    assert_eq!(bodies[1]["usage"]["records"].as_array().unwrap().len(), 1);
    assert_eq!(bodies[1]["segments"], json!([]));
    assert!(!f.store.preview(SESSION).unwrap()[0].needs_attention);
}

#[test]
fn first_enabled_prompt_baseline_excludes_preconsent_history_without_numeric_storage() {
    let mut f = Fixture::new();
    let mut old = request("msg_before_consent", 5);
    old["message"]["usage"]["input_tokens"] = json!(999123457u64);
    f.write(vec![old.clone()]);
    f.event(Prompt, 10, true);
    let db = fs::read(f.dir.path().join("capture/capture.sqlite3")).unwrap();
    assert!(!String::from_utf8_lossy(&db).contains("999123457"));
    f.write(vec![old, request("msg_after_consent", 7)]);
    f.event(Stop, 20, true);
    assert_eq!(f.usage_ids(), vec!["msg_after_consent"]);
}
#[test]
fn paused_terminals_collect_nothing_and_resume_requires_a_new_prompt_baseline() {
    let mut f = Fixture::new();
    f.start(0);
    f.store.control(&repo(), "pause", &clock(30)).unwrap();
    let paused = request("msg_created_while_paused", 5);
    f.write(vec![paused.clone()]);
    f.event(Stop, 100, true);
    assert!(f.usage_ids().is_empty());
    let db = fs::read(f.dir.path().join("capture/capture.sqlite3")).unwrap();
    assert!(!String::from_utf8_lossy(&db).contains("msg_created_while_paused"));
    f.store.control(&repo(), "resume", &clock(110)).unwrap();
    f.event(Stop, 150, true);
    assert!(f.usage_ids().is_empty());
    f.event_turn(Prompt, 200, true, "prompt_2");
    f.write(vec![paused, request("msg_new_enabled_turn", 6)]);
    f.event_turn(Stop, 250, true, "prompt_2");
    assert_eq!(f.usage_ids(), vec!["msg_new_enabled_turn"]);
}
#[test]
fn preconsent_provisional_id_stays_excluded_when_its_final_record_arrives() {
    let mut f = Fixture::new();
    let mut provisional = request("msg_existing_provisional", 0);
    provisional["message"]["stop_reason"] = Value::Null;
    f.write(vec![provisional]);
    f.event(Prompt, 0, true);
    f.write(vec![
        request("msg_existing_provisional", 5),
        request("msg_new_request", 6),
    ]);
    f.event(Stop, 10, true);
    assert_eq!(f.usage_ids(), vec!["msg_new_request"]);
}
#[test]
fn missing_prompt_baseline_cannot_be_reconstructed_from_a_later_terminal_snapshot() {
    let mut f = Fixture::new();
    f.event(Prompt, 0, true);
    f.write(vec![request("msg_unknown_epoch", 5)]);
    f.event(Stop, 10, true);
    assert!(f.usage_ids().is_empty());
    f.event_turn(Prompt, 20, true, "prompt_2");
    f.write(vec![
        request("msg_unknown_epoch", 5),
        request("msg_baselined_epoch", 6),
    ]);
    f.event_turn(Stop, 30, true, "prompt_2");
    assert_eq!(f.usage_ids(), vec!["msg_baselined_epoch"]);
}
#[test]
fn terminal_capability_without_prompt_capability_never_retroactively_imports_usage() {
    let mut f = Fixture::new();
    f.write(vec![]);
    f.event(Prompt, 0, false);
    f.write(vec![request("msg_not_authorized_at_prompt", 5)]);
    f.event(Stop, 10, true);
    assert!(f.usage_ids().is_empty());
}
#[test]
fn incomplete_prompt_baseline_requires_another_new_prompt_not_terminal_repair() {
    let mut f = Fixture::new();
    fs::write(&f.path, "{\"type\":\"assistant\",\"sessionId\":").unwrap();
    f.event(Prompt, 0, true);
    f.write(vec![request("msg_after_unsafe_baseline", 5)]);
    f.event(Stop, 10, true);
    assert!(f.usage_ids().is_empty());
    f.event_turn(Prompt, 20, true, "prompt_2");
    f.write(vec![
        request("msg_after_unsafe_baseline", 5),
        request("msg_after_complete_baseline", 6),
    ]);
    f.event_turn(Stop, 30, true, "prompt_2");
    assert_eq!(f.usage_ids(), vec!["msg_after_complete_baseline"]);
}

#[test]
fn prior_accepted_usage_is_immutable_and_not_duplicated_across_pause_baselines() {
    let mut f = Fixture::new();
    f.start(0);
    let accepted = request("msg_already_accepted", 5);
    f.write(vec![accepted.clone()]);
    f.event(Stop, 10, true);
    let frozen = f.bodies()[0].clone();
    f.store.control(&repo(), "pause", &clock(20)).unwrap();
    let excluded = request("msg_paused_after_accepted", 6);
    f.write(vec![accepted.clone(), excluded.clone()]);
    f.event(Stop, 30, true);
    assert_eq!(f.bodies()[0], frozen);
    f.store.control(&repo(), "resume", &clock(40)).unwrap();
    f.event(Stop, 50, true);
    assert_eq!(f.usage_ids(), vec!["msg_already_accepted"]);
    f.event_turn(Prompt, 60, true, "prompt_2");
    f.write(vec![
        accepted,
        excluded,
        request("msg_after_new_baseline", 7),
    ]);
    f.event_turn(Stop, 70, true, "prompt_2");
    assert_eq!(
        f.usage_ids(),
        vec!["msg_already_accepted", "msg_after_new_baseline"]
    );
    assert_eq!(f.bodies()[0], frozen);
}
#[test]
fn repo_global_and_harness_revocation_reset_usage_even_for_an_already_closed_turn() {
    for mode in ["repo", "global", "harness"] {
        let mut f = Fixture::new();
        f.start(0);
        let first = request("msg_prior_approved", 5);
        f.write(vec![first.clone()]);
        f.event(Stop, 10, true);
        match mode {
            "repo" => f.store.control(&repo(), "disable", &clock(20)).unwrap(),
            "global" => f.store.disable_all().unwrap(),
            _ => {
                let mut removed = f.binding.clone();
                removed.agents.clear();
                f.store.approve(removed).unwrap();
            }
        }
        let excluded = request("msg_revoked_period", 6);
        f.write(vec![first.clone(), excluded.clone()]);
        f.store.approve(f.binding.clone()).unwrap();
        f.event(Stop, 40, true);
        assert_eq!(f.usage_ids(), vec!["msg_prior_approved"], "{mode}");
        f.event_turn(Prompt, 50, true, "prompt_2");
        f.write(vec![first, excluded, request("msg_reapproved_new_turn", 7)]);
        f.event_turn(Stop, 60, true, "prompt_2");
        assert_eq!(
            f.usage_ids(),
            vec!["msg_prior_approved", "msg_reapproved_new_turn"],
            "{mode}"
        );
    }
}
#[test]
fn a_duplicate_prompt_cannot_retroactively_establish_the_missing_usage_baseline() {
    let mut f = Fixture::new();
    f.write(vec![]);
    f.event(Prompt, 0, false);
    f.write(vec![request("msg_before_late_prompt_replay", 5)]);
    f.event(Prompt, 5, true);
    f.event(Stop, 10, true);
    assert!(f.usage_ids().is_empty());
}

#[test]
fn changed_previously_accepted_request_still_holds_after_a_new_consent_baseline() {
    let mut f = Fixture::new();
    f.start(0);
    f.write(vec![request("msg_accepted_before_pause", 5)]);
    f.event(Stop, 10, true);
    let frozen = f.bodies()[0].clone();
    f.store.control(&repo(), "pause", &clock(20)).unwrap();
    f.store.control(&repo(), "resume", &clock(30)).unwrap();
    f.event_turn(Prompt, 40, true, "prompt_2");
    f.write(vec![
        request("msg_accepted_before_pause", 6),
        request("msg_new_but_conflicted_snapshot", 7),
    ]);
    f.event_turn(Stop, 50, true, "prompt_2");
    assert!(f.store.preview(SESSION).unwrap()[0].needs_attention);
    assert!(f.store.claim(&f.binding, clock(60).wall).unwrap().is_none());
    assert_eq!(f.bodies()[0], frozen);
    assert_eq!(f.usage_ids(), vec!["msg_accepted_before_pause"]);
}
#[test]
fn excluded_history_never_decodes_or_stores_old_numeric_model_or_finality_payloads() {
    let mut f = Fixture::new();
    let mut old = request("msg_history_identity_only", 5);
    old["message"]["usage"]["input_tokens"] = json!("PRIVATE_NON_NUMERIC");
    old["message"]["model"] = json!("/PRIVATE_OLD_MODEL");
    old["message"]["stop_reason"] = json!("unqualified_old_reason");
    f.write(vec![old.clone()]);
    f.event(Prompt, 0, true);
    f.write(vec![old, request("msg_new_valid_payload", 6)]);
    f.event(Stop, 10, true);
    assert_eq!(f.usage_ids(), vec!["msg_new_valid_payload"]);
    let bytes = fs::read(f.dir.path().join("capture/capture.sqlite3")).unwrap();
    let persisted = String::from_utf8_lossy(&bytes);
    assert!(!persisted.contains("PRIVATE_NON_NUMERIC"));
    assert!(!persisted.contains("PRIVATE_OLD_MODEL"));
}

#[test]
fn codex_prompt_baseline_and_pause_epoch_exclude_old_response_ids() {
    let mut f = Fixture::new();
    f.namespace = "codex";
    f.version = "0.167.0";
    let mut binding = f.binding.clone();
    binding.agents = vec!["codex".into()];
    f.binding = f.store.approve(binding).unwrap();
    f.path = f.home.join(format!(
        ".codex/sessions/2026/11/08/rollout-2026-11-08-{SESSION}.jsonl"
    ));
    fs::create_dir_all(f.path.parent().unwrap()).unwrap();
    let meta = json!({"type":"session_meta","payload":{"id":SESSION,"session_id":SESSION,"cli_version":"0.167.0"}});
    let context = |turn: &str| json!({"type":"turn_context","payload":{"turn_id":turn,"model":"gpt-6.1-sol"}});
    let response = |id: &str, turn: &str| json!({"type":"token_usage_record","payload":{"thread_id":SESSION,"session_id":SESSION,"turn_id":turn,"root_turn_id":turn,"response_id":id,"usage":{"input_tokens":101,"cached_input_tokens":80,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":2}}});
    let mut provisional = response("resp_before_consent", "old_turn");
    provisional["payload"]["usage"] = Value::Null;
    f.write(vec![meta.clone(), context("old_turn"), provisional]);
    f.event(Prompt, 0, true);
    let old = response("resp_before_consent", "old_turn");
    let accepted = response("resp_after_consent", "prompt_1");
    f.write(vec![
        meta.clone(),
        context("old_turn"),
        old.clone(),
        context("prompt_1"),
        accepted.clone(),
    ]);
    f.event(Stop, 10, true);
    assert_eq!(f.usage_ids(), vec!["resp_after_consent"]);
    f.store.control(&repo(), "pause", &clock(20)).unwrap();
    let excluded = response("resp_paused", "paused_turn");
    f.write(vec![
        meta.clone(),
        context("old_turn"),
        old.clone(),
        context("prompt_1"),
        accepted.clone(),
        context("paused_turn"),
        excluded.clone(),
    ]);
    f.event(Stop, 30, true);
    f.store.control(&repo(), "resume", &clock(40)).unwrap();
    f.event(Stop, 50, true);
    assert_eq!(f.usage_ids(), vec!["resp_after_consent"]);
    f.event_turn(Prompt, 60, true, "prompt_2");
    f.write(vec![
        meta,
        context("old_turn"),
        old,
        context("prompt_1"),
        accepted,
        context("paused_turn"),
        excluded,
        context("prompt_2"),
        response("resp_new_epoch", "prompt_2"),
    ]);
    f.event_turn(Stop, 70, true, "prompt_2");
    assert_eq!(f.usage_ids(), vec!["resp_after_consent", "resp_new_epoch"]);
    let bodies = f.bodies();
    assert_eq!(
        bodies.last().unwrap()["usage"]["records"][0]["input_basis"],
        "CODEX_TOTAL_INPUT"
    );
    assert_eq!(
        bodies.last().unwrap()["usage"]["records"][0]["cached_input_tokens"],
        80
    );
}
