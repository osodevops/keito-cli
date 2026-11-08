//! The numeric shapes/counts below are minimized actual 2026-11-08 probe
//! observations. Envelopes reconstruct allowlisted fields from the initial and
//! envelope-session probes; raw content was discarded. Malformed/stream variants
//! and edge cases are synthetic validation, not harness qualification evidence.
#[path = "../src/capture_usage.rs"]
mod capture_usage;
use capture_usage::*;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

const SESSION: &str = "59455d61-5c59-4419-9cb8-9fb7143c317d";
struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    path: PathBuf,
    harness: Harness,
}
impl Fixture {
    fn new(harness: Harness, records: Vec<Value>) -> Self {
        // /tmp is commonly a symlink on macOS; policy requires resolved HOME.
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap().join("home");
        let path = match harness {
            Harness::ClaudeCode => home.join(format!(".claude/projects/synthetic/{SESSION}.jsonl")),
            Harness::Codex => home.join(format!(
                ".codex/sessions/2026/11/08/rollout-2026-11-08T12-00-00-{SESSION}.jsonl"
            )),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let this = Self {
            _temp: temp,
            home,
            path,
            harness,
        };
        this.write(records);
        this
    }
    fn write(&self, records: Vec<Value>) {
        fs::write(
            &self.path,
            records
                .iter()
                .map(|r| format!("{}\n", r))
                .collect::<String>(),
        )
        .unwrap();
    }
    fn approved(&self) -> ApprovedActiveSession {
        ApprovedActiveSession::new(
            self.harness,
            if self.harness == Harness::Codex {
                "0.167.0"
            } else {
                "2.1.319"
            },
            &self.home,
            &self.path,
            SESSION,
        )
        .unwrap()
    }
    fn read(&self) -> Result<UsageEvidence, UsageReadError> {
        read_active_session(&self.approved())
    }
}
fn claude(id: &str, output: u64) -> Value {
    json!({"type":"assistant","sessionId":SESSION,"message":{"id":id,"model":"claude-opus-5-5","stop_reason":"end_turn","usage":{"input_tokens":2,"cache_creation_input_tokens":2280,"cache_read_input_tokens":0,"output_tokens":output},"content":[{"text":"PRIVATE_PROMPT_CANARY /private/path sk-secret"}]}})
}
fn meta() -> Value {
    json!({"type":"session_meta","payload":{"id":SESSION,"session_id":SESSION,"cli_version":"0.167.0","cwd":"PRIVATE_PATH_CANARY"}})
}
fn context(turn: &str) -> Value {
    json!({"type":"turn_context","payload":{"turn_id":turn,"model":"gpt-6.1-sol","instructions":"PRIVATE_INSTRUCTION_CANARY"}})
}
fn codex(id: &str, turn: &str, resumed: bool) -> Value {
    let (input, cached) = if resumed {
        (20333, 20096)
    } else {
        (20309, 19328)
    };
    json!({"type":"token_usage_record","payload":{"thread_id":SESSION,"session_id":SESSION,"turn_id":turn,"root_turn_id":turn,"response_id":id,"usage":{"input_tokens":input,"cached_input_tokens":cached,"cache_write_input_tokens":0,"output_tokens":6,"reasoning_output_tokens":0,"total_tokens":input+6},"thread_token_usage":{"input_tokens":40642,"output_tokens":12},"turn_token_usage":{"input_tokens":999999}}})
}
fn lifecycle(kind: &str, turn: &str) -> Value {
    json!({"type":"event_msg","payload":{"type":kind,"turn_id":turn,"message":"PRIVATE_EVENT_CANARY"}})
}

#[test]
fn observed_claude_tool_turn_and_model_change_retain_three_request_deltas() {
    // Exact observed counts/IDs; the approved test session envelope is synthetic.
    let observations = [
        (
            "msg_011CgqR9eX293BGPxqbyYPK6",
            "claude-opus-5-5",
            "tool_use",
            2,
            4224,
            0,
            81,
        ),
        (
            "msg_011CgqR9p5zFhFpDa2WMG2VG",
            "claude-opus-5-5",
            "end_turn",
            2,
            125,
            4224,
            5,
        ),
        (
            "msg_011CgqR9whPUXYvaWyNJXFaj",
            "claude-sonnet-5-5",
            "end_turn",
            2,
            4665,
            0,
            7,
        ),
    ];
    let rows = observations.into_iter().map(|(id,model,stop,input,write,read,output)| {
        json!({"type":"assistant","sessionId":SESSION,"message":{"id":id,"model":model,"stop_reason":stop,
            "usage":{"input_tokens":input,"cache_creation_input_tokens":write,"cache_read_input_tokens":read,"output_tokens":output},
            "content":[{"type":if stop=="tool_use" {"tool_use"} else {"text"},"text":"PRIVATE_PROSE_CANARY"}]}})
    }).collect();
    let f = Fixture::new(Harness::ClaudeCode, rows);
    let e = f.read().unwrap();
    assert_eq!(e.requests.len(), 3);
    assert_eq!(
        e.requests
            .iter()
            .map(|r| r.output_tokens.unwrap())
            .sum::<u64>(),
        93
    );
    assert_eq!(
        e.requests
            .iter()
            .map(|r| r.cached_input_tokens.unwrap())
            .sum::<u64>(),
        4224
    );
    assert_eq!(
        e.requests
            .iter()
            .filter(|r| r.model.as_deref() == Some("claude-sonnet-5-5"))
            .count(),
        1
    );
    assert_eq!(e.coverage, "PARTIAL");
    assert!(!serde_json::to_string(&e).unwrap().contains("PRIVATE"));
}

#[test]
fn stale_or_missing_codex_source_version_and_missing_native_id_are_refused() {
    let f = Fixture::new(Harness::Codex, vec![]);
    for version in [json!("0.166.0"), Value::Null] {
        let mut record = meta();
        record["payload"]["cli_version"] = version;
        f.write(vec![
            record,
            context("turn_1"),
            codex("resp_1", "turn_1", false),
        ]);
        assert_eq!(f.read(), Err(UsageReadError::UnsupportedVersion));
    }
    let mut record = meta();
    record["payload"]
        .as_object_mut()
        .unwrap()
        .remove("session_id");
    f.write(vec![
        record,
        context("turn_1"),
        codex("resp_1", "turn_1", false),
    ]);
    assert_eq!(f.read(), Err(UsageReadError::InvalidSession));
}

#[test]
fn unsupported_nonnull_claude_finality_remains_explicitly_held() {
    for stop in [json!("unknown"), json!("max_tokens")] {
        let mut partial = claude("msg_1", 5);
        partial["message"]["stop_reason"] = stop;
        let f = Fixture::new(
            Harness::ClaudeCode,
            vec![partial, claude("msg_1", 5), claude("msg_2", 3)],
        );
        let e = f.read().unwrap();
        assert_eq!(e.requests.len(), 1);
        assert_eq!(e.requests[0].source_record_id, "msg_2");
        assert!(e.limitations.contains(&"UNKNOWN_REQUEST_FINALITY"));
    }
}

#[test]
fn provisional_claude_frames_yield_to_one_qualified_final_record() {
    // Synthetic stream ordering exercises the proposed contract; the real
    // pinned probe did not observe same-ID provisional-to-final delivery.
    for absent in [false, true] {
        for final_first in [false, true] {
            let mut provisional = claude("msg_1", 1);
            provisional["message"]["stop_reason"] = Value::Null;
            if absent {
                provisional["message"]
                    .as_object_mut()
                    .unwrap()
                    .remove("stop_reason");
            }
            let final_record = claude("msg_1", 81);
            let rows = if final_first {
                vec![
                    final_record.clone(),
                    provisional.clone(),
                    provisional,
                    final_record,
                ]
            } else {
                vec![
                    provisional.clone(),
                    provisional,
                    final_record.clone(),
                    final_record,
                ]
            };
            let f = Fixture::new(Harness::ClaudeCode, rows);
            let e = f.read().unwrap();
            assert_eq!(e.requests.len(), 1);
            assert_eq!(e.requests[0].output_tokens, Some(81));
            assert!(!e.limitations.contains(&"UNKNOWN_REQUEST_FINALITY"));
            assert!(!e.limitations.contains(&"PENDING_REQUEST_FINALITY"));
            assert!(!e.limitations.contains(&"CONFLICTING_REQUEST_RECORDS"));
            assert_eq!(e.coverage, "PARTIAL");
        }
    }
}

#[test]
fn provisional_only_claude_request_is_pending_and_can_resolve_in_a_later_snapshot() {
    let mut provisional = claude("msg_1", 1);
    provisional["message"]["stop_reason"] = Value::Null;
    let f = Fixture::new(
        Harness::ClaudeCode,
        vec![provisional.clone(), claude("msg_2", 3)],
    );
    let pending = f.read().unwrap();
    assert_eq!(pending.requests.len(), 1);
    assert_eq!(pending.requests[0].source_record_id, "msg_2");
    assert!(pending.limitations.contains(&"PENDING_REQUEST_FINALITY"));
    assert!(!pending.limitations.contains(&"UNKNOWN_REQUEST_FINALITY"));
    f.write(vec![provisional, claude("msg_1", 81), claude("msg_2", 3)]);
    let resolved = f.read().unwrap();
    assert_eq!(resolved.requests.len(), 2);
    assert_eq!(
        resolved
            .requests
            .iter()
            .map(|r| r.output_tokens.unwrap())
            .sum::<u64>(),
        84
    );
    assert!(!resolved.limitations.contains(&"PENDING_REQUEST_FINALITY"));
    assert_eq!(resolved.coverage, "PARTIAL");
}

#[test]
fn observed_codex_abort_without_usage_is_unknown_not_zero() {
    let mut record = codex(
        "resp_08030d9a2a9f256b016af0634fb21487d28fc0d98709899918",
        "turn_1",
        false,
    );
    record["payload"]["usage"]["input_tokens"] = json!(17975);
    record["payload"]["usage"]["cached_input_tokens"] = json!(0);
    record["payload"]["usage"]["total_tokens"] = json!(17981);
    let f = Fixture::new(
        Harness::Codex,
        vec![
            meta(),
            context("turn_1"),
            lifecycle("task_started", "turn_1"),
            record,
            lifecycle("task_complete", "turn_1"),
            context("turn_2"),
            lifecycle("task_started", "turn_2"),
            lifecycle("turn_aborted", "turn_2"),
        ],
    );
    let e = f.read().unwrap();
    assert_eq!(e.requests.len(), 1);
    assert_eq!(e.requests[0].input_tokens, Some(17975));
    assert!(e.limitations.contains(&"INTERRUPTED_TURN"));
    assert!(e.limitations.contains(&"MISSING_TURN_USAGE"));
    assert_eq!(e.coverage, "PARTIAL");
    assert!(!serde_json::to_string(&e).unwrap().contains("PRIVATE"));
}

#[test]
fn actual_claude_shape_preserves_raw_input_and_cache_without_prose() {
    let f = Fixture::new(
        Harness::ClaudeCode,
        vec![claude("msg_011CgqP5w7Za31Gd1RKqi4wG", 5)],
    );
    let evidence = f.read().unwrap();
    assert_eq!(
        (evidence.coverage, evidence.coverage_reason),
        ("PARTIAL", "OBSERVED_REQUEST_RECORDS")
    );
    assert_eq!(evidence.requests.len(), 1);
    let r = &evidence.requests[0];
    assert_eq!(r.input_basis, "CLAUDE_UNCACHED_INPUT");
    assert_eq!(
        (
            r.input_tokens,
            r.cached_input_tokens,
            r.cache_write_input_tokens,
            r.output_tokens
        ),
        (Some(2), Some(0), Some(2280), Some(5))
    );
    assert_eq!(r.reasoning_output_tokens, None);
    assert_eq!(r.model_provenance, "RESPONSE_MODEL");
    let serialized = serde_json::to_string(&evidence).unwrap();
    assert!(!serialized.contains("PRIVATE"));
    assert!(!serialized.contains("sk-secret"));
}
#[test]
fn codex_resume_uses_response_deltas_not_cumulative_totals() {
    let f = Fixture::new(
        Harness::Codex,
        vec![
            meta(),
            context("turn_1"),
            codex("resp_1", "turn_1", false),
            context("turn_2"),
            codex("resp_2", "turn_2", true),
        ],
    );
    let e = f.read().unwrap();
    assert_eq!(e.requests.len(), 2);
    assert_eq!(
        e.requests
            .iter()
            .map(|r| r.input_tokens.unwrap())
            .sum::<u64>(),
        40642
    );
    assert_eq!(e.requests[1].cached_input_tokens, Some(20096));
    assert_eq!(e.requests[0].input_basis, "CODEX_TOTAL_INPUT");
    assert_eq!(e.requests[0].model_provenance, "TURN_CONFIG");
    assert!(!serde_json::to_string(&e).unwrap().contains("999999"));
}
#[test]
fn identical_terminal_delivery_is_deduped_but_conflicting_terminal_records_are_held() {
    let f = Fixture::new(
        Harness::ClaudeCode,
        vec![claude("msg_1", 5), claude("msg_1", 5)],
    );
    assert_eq!(f.read().unwrap().requests.len(), 1);
    f.write(vec![
        claude("msg_1", 1),
        claude("msg_1", 5),
        claude("msg_2", 3),
    ]);
    let e = f.read().unwrap();
    assert_eq!(e.requests.len(), 1);
    assert_eq!(e.requests[0].source_record_id, "msg_2");
    assert!(e.limitations.contains(&"CONFLICTING_REQUEST_RECORDS"));
}
#[test]
fn incomplete_trailing_json_is_never_an_observation() {
    let f = Fixture::new(Harness::ClaudeCode, vec![claude("msg_1", 5)]);
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&f.path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\"")
        .unwrap();
    let e = f.read().unwrap();
    assert_eq!(e.requests.len(), 1);
    assert!(e.limitations.contains(&"INCOMPLETE_TAIL"));
}
#[test]
fn wrong_or_absent_session_identity_fails_closed() {
    let mut wrong = claude("msg_1", 5);
    wrong["sessionId"] = json!("other-session");
    let f = Fixture::new(Harness::ClaudeCode, vec![wrong]);
    assert_eq!(f.read(), Err(UsageReadError::InvalidSession));
    let mut absent = claude("msg_1", 5);
    absent.as_object_mut().unwrap().remove("sessionId");
    f.write(vec![absent]);
    assert_eq!(f.read(), Err(UsageReadError::InvalidSession));
    let f = Fixture::new(
        Harness::Codex,
        vec![context("turn_1"), codex("resp_1", "turn_1", false)],
    );
    assert_eq!(f.read(), Err(UsageReadError::InvalidSession));
    let mut wrong = codex("resp_1", "turn_1", false);
    wrong["payload"]["thread_id"] = json!("other-session");
    f.write(vec![meta(), context("turn_1"), wrong]);
    assert_eq!(f.read(), Err(UsageReadError::InvalidSession));
}
#[test]
fn stale_version_and_repository_controlled_paths_are_refused() {
    let f = Fixture::new(Harness::ClaudeCode, vec![]);
    assert!(matches!(
        ApprovedActiveSession::new(f.harness, "2.1.318", &f.home, &f.path, SESSION),
        Err(UsageReadError::UnsupportedVersion)
    ));
    assert!(matches!(
        ApprovedActiveSession::new(
            f.harness,
            "2.1.319",
            &f.home,
            &f.home.join(format!("repo/{SESSION}.jsonl")),
            SESSION
        ),
        Err(UsageReadError::UnsafePath)
    ));
}

#[test]
fn absent_counters_stay_unknown_and_explicit_zero_is_preserved() {
    let mut record = claude("msg_1", 0);
    record["message"]["usage"]
        .as_object_mut()
        .unwrap()
        .remove("cache_read_input_tokens");
    let f = Fixture::new(Harness::ClaudeCode, vec![record]);
    let r = f.read().unwrap().requests.remove(0);
    assert_eq!(r.cached_input_tokens, None);
    assert_eq!(r.output_tokens, Some(0));
    let mut record = codex("resp_1", "turn_1", false);
    record["payload"]["usage"] = Value::Null;
    let f = Fixture::new(Harness::Codex, vec![meta(), record]);
    let e = f.read().unwrap();
    assert!(e.requests.is_empty());
    assert!(e.limitations.contains(&"MISSING_USAGE"));
    assert!(e.limitations.contains(&"MISSING_MODEL"));
}

#[test]
fn invalid_numeric_categories_and_subset_violations_are_refused() {
    for value in [
        json!(-1),
        json!(1.5),
        json!("5"),
        json!(1_000_000_000_001u64),
    ] {
        let mut record = codex("resp_1", "turn_1", false);
        record["payload"]["usage"]["input_tokens"] = value;
        let f = Fixture::new(Harness::Codex, vec![meta(), record]);
        assert_eq!(f.read(), Err(UsageReadError::InvalidRecord));
    }
    for (field, value) in [
        ("cached_input_tokens", 20310),
        ("reasoning_output_tokens", 7),
    ] {
        let mut record = codex("resp_1", "turn_1", false);
        record["payload"]["usage"][field] = json!(value);
        let f = Fixture::new(Harness::Codex, vec![meta(), record]);
        assert_eq!(f.read(), Err(UsageReadError::InvalidRecord));
    }
    // A real observed shape had reasoning 16 inside output 23. Never sum it.
    let mut record = codex("resp_1", "turn_1", false);
    record["payload"]["usage"]["output_tokens"] = json!(23);
    record["payload"]["usage"]["reasoning_output_tokens"] = json!(16);
    let f = Fixture::new(Harness::Codex, vec![meta(), context("turn_1"), record]);
    let r = f.read().unwrap().requests.remove(0);
    assert_eq!(
        (r.output_tokens, r.reasoning_output_tokens),
        (Some(23), Some(16))
    );
}

#[test]
fn decreasing_thread_snapshots_do_not_reset_request_deltas() {
    let first = codex("resp_1", "turn_1", false);
    let mut second = codex("resp_2", "turn_2", true);
    second["payload"]["thread_token_usage"] = json!({"input_tokens":0});
    second["payload"]["turn_token_usage"] = json!({"input_tokens":0});
    let f = Fixture::new(
        Harness::Codex,
        vec![meta(), context("turn_1"), first, context("turn_2"), second],
    );
    assert_eq!(
        f.read()
            .unwrap()
            .requests
            .iter()
            .map(|r| r.input_tokens.unwrap())
            .sum::<u64>(),
        40642
    );
}

#[test]
fn repeated_equal_responses_with_different_ids_remain_distinct() {
    let f = Fixture::new(
        Harness::Codex,
        vec![
            meta(),
            context("turn_1"),
            codex("resp_1", "turn_1", false),
            codex("resp_2", "turn_1", false),
        ],
    );
    let e = f.read().unwrap();
    assert_eq!(e.requests.len(), 2);
    assert_eq!(
        e.requests
            .iter()
            .map(|r| r.output_tokens.unwrap())
            .sum::<u64>(),
        12
    );
}

#[test]
fn missing_snapshot_for_an_existing_id_is_a_conflict_not_silent_fallback() {
    let first = codex("resp_1", "turn_1", false);
    let mut missing = first.clone();
    missing["payload"]["usage"] = Value::Null;
    let f = Fixture::new(
        Harness::Codex,
        vec![meta(), context("turn_1"), first, missing],
    );
    let e = f.read().unwrap();
    assert!(e.requests.is_empty());
    assert!(e.limitations.contains(&"CONFLICTING_REQUEST_RECORDS"));
}

#[test]
fn serialized_request_has_only_exact_approved_wire_fields() {
    let f = Fixture::new(Harness::ClaudeCode, vec![claude("msg_1", 5)]);
    let e = f.read().unwrap();
    let value = serde_json::to_value(&e.requests[0]).unwrap();
    let fields: std::collections::BTreeSet<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        [
            "source_adapter",
            "source_record_id",
            "source_turn_id",
            "model",
            "model_provenance",
            "counter_semantics",
            "input_basis",
            "input_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "output_tokens",
            "reasoning_output_tokens"
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(value["source_adapter"], json!("CLAUDE_JSONL_2_1_319_V1"));
    assert!(value.get("total_tokens").is_none());
    assert!(value.get("cost").is_none());
}

#[test]
fn same_turn_conflicting_model_never_becomes_a_last_wins_attribution() {
    let mut changed = context("turn_1");
    changed["payload"]["model"] = json!("gpt-other");
    let f = Fixture::new(
        Harness::Codex,
        vec![
            meta(),
            context("turn_1"),
            codex("resp_1", "turn_1", false),
            changed,
            context("turn_1"),
        ],
    );
    let e = f.read().unwrap();
    assert_eq!(e.requests[0].model, None);
    assert_eq!(e.requests[0].model_provenance, "UNKNOWN");
    assert!(e.limitations.contains(&"CONFLICTING_TURN_MODELS"));
}

#[test]
fn duplicate_known_fields_malformed_lines_and_secret_ids_have_static_errors() {
    let f = Fixture::new(Harness::ClaudeCode, vec![]);
    for content in [
        format!("{{\"type\":\"assistant\",\"sessionId\":\"other\",\"sessionId\":\"{SESSION}\"}}\n"),
        "{PRIVATE_RAW_CANARY\n".to_owned(),
        format!("{{\"type\":\"assistant\",\"sessionId\":\"{SESSION}\",\"message\":{{\"id\":\"sk-SECRET_CANARY\"}}}}\n"),
    ] {
        fs::write(&f.path,content).unwrap();
        let error = f.read().unwrap_err();
        assert!(!format!("{error} {error:?}").contains("CANARY"));
        assert!(!format!("{error}").contains(&f.home.to_string_lossy().to_string()));
    }
}

#[test]
fn native_metadata_and_context_identity_are_verified() {
    let mut bad_meta = meta();
    bad_meta["payload"]["id"] = json!("other-session");
    let f = Fixture::new(Harness::Codex, vec![bad_meta]);
    assert_eq!(f.read(), Err(UsageReadError::InvalidSession));
    let mut bad_context = context("turn_1");
    bad_context["payload"]["session_id"] = json!("other-session");
    f.write(vec![meta(), bad_context, codex("resp_1", "turn_1", false)]);
    assert_eq!(f.read(), Err(UsageReadError::InvalidSession));
}

#[test]
fn size_record_count_and_deep_json_are_bounded() {
    let f = Fixture::new(Harness::ClaudeCode, vec![]);
    fs::write(&f.path, vec![b' '; MAX_SNAPSHOT_BYTES + 1]).unwrap();
    assert_eq!(f.read(), Err(UsageReadError::TooLarge));
    fs::write(&f.path, b"\n".repeat(4097)).unwrap();
    assert_eq!(f.read(), Err(UsageReadError::TooLarge));
    fs::write(
        &f.path,
        format!(
            "{{\"type\":\"ignored\",\"unknown\":{}0{}}}\n",
            "[".repeat(300),
            "]".repeat(300)
        ),
    )
    .unwrap();
    assert_eq!(f.read(), Err(UsageReadError::InvalidRecord));
}

#[cfg(unix)]
#[test]
fn symlinked_leaf_and_any_parent_are_refused_without_reading_target() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(Harness::ClaudeCode, vec![claude("msg_1", 5)]);
    let approved = f.approved();
    let elsewhere = f.home.join("unapproved.jsonl");
    fs::rename(&f.path, &elsewhere).unwrap();
    symlink(&elsewhere, &f.path).unwrap();
    assert_eq!(
        read_active_session(&approved),
        Err(UsageReadError::UnsafePath)
    );
    fs::remove_file(&f.path).unwrap();
    fs::rename(&elsewhere, &f.path).unwrap();
    let original_parent = f.path.parent().unwrap();
    let moved_parent = f.home.join("moved-project");
    fs::rename(original_parent, &moved_parent).unwrap();
    symlink(&moved_parent, original_parent).unwrap();
    assert_eq!(
        read_active_session(&approved),
        Err(UsageReadError::UnsafePath)
    );
}

#[cfg(unix)]
#[test]
fn fifo_directory_and_hardlink_are_refused() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new(Harness::ClaudeCode, vec![]);
    let approved = f.approved();
    fs::remove_file(&f.path).unwrap();
    let name = CString::new(f.path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let started = std::time::Instant::now();
    assert_eq!(
        read_active_session(&approved),
        Err(UsageReadError::UnsafePath)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    fs::remove_file(&f.path).unwrap();
    fs::create_dir(&f.path).unwrap();
    assert_eq!(
        read_active_session(&approved),
        Err(UsageReadError::UnsafePath)
    );
    fs::remove_dir(&f.path).unwrap();
    f.write(vec![claude("msg_1", 5)]);
    fs::hard_link(&f.path, f.home.join("alias.jsonl")).unwrap();
    assert_eq!(
        read_active_session(&approved),
        Err(UsageReadError::UnsafePath)
    );
}
