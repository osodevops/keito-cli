//! Normal command acceptance, not a synthetic UsageSource approval path.
//! The first selected status case is intentionally RED on 3ac: `usage` is absent.
//! Source-fixture paired numeric positives are a separate subsequent gate.
#![cfg(unix)]
#[path = "support/mutation_environment.rs"]
mod isolation;
#[path = "support/service_boundary.rs"]
mod service_boundary;

use keito_cli::capture::{self, Binding, Store};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Output,
};

const SESSION: &str = "12345678-1234-4234-8234-123456789abc";
const NAMESPACES: [(&str, &str); 2] = [("claude-code", "2.1.319"), ("codex", "0.167.0")];

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    capture: PathBuf,
    repo: PathBuf,
    core: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let capture = home.join("capture");
        let repo = root.join("repo");
        for path in [
            home.clone(),
            capture.clone(),
            home.join(".claude"),
            home.join(".codex"),
            home.join("config"),
            home.join("AppData"),
            home.join("state"),
            root.join("bin"),
            root.join("tmp"),
            root.join("escape"),
            repo.clone(),
        ] {
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let core = root.join("keito-source-cli");
        fs::copy(assert_cmd::cargo::cargo_bin!("keito"), &core).unwrap();
        fs::set_permissions(&core, fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            home,
            capture,
            repo,
            core,
        };
        fixture.boundary_negatives_before_any_command();

        // Only Git exists on this explicit PATH. No real harness, service or
        // credential helper can accidentally be selected from the operator PATH.
        symlink("/usr/bin/git", fixture.root.join("bin/git")).unwrap();
        let git = fixture
            .command(Path::new("/usr/bin/git"))
            .args(["init", "--quiet"])
            .output()
            .unwrap();
        assert!(git.status.success(), "private Git setup failed");
        let mut store = Store::open(&fixture.capture).unwrap();
        store
            .approve(Binding {
                repository_key: capture::hash(
                    fixture.repo.join(".git").to_string_lossy().as_bytes(),
                ),
                remote_hash: capture::hash(""),
                hint_hash: capture::hash("no-mapping-hint"),
                id: String::new(),
                version: 0,
                workspace_id: "co_synthetic".into(),
                owner_id: "user_synthetic".into(),
                origin_hash: capture::hash("https://example.invalid"),
                project_id: "project_synthetic".into(),
                task_id: "task_synthetic".into(),
                agents: NAMESPACES.iter().map(|(n, _)| (*n).into()).collect(),
                paused: false,
                revoked: false,
            })
            .unwrap();
        drop(store);
        fixture
    }

    fn checked(&self) -> isolation::CheckedEnvironment {
        for path in [
            &self.root,
            &self.home,
            &self.capture,
            &self.repo,
            &self.root.join("tmp"),
        ] {
            let metadata = fs::symlink_metadata(path).unwrap();
            assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
            assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
            assert_eq!(metadata.mode() & 0o777, 0o700);
            assert_eq!(path.canonicalize().unwrap(), *path);
        }
        isolation::checked(
            &self.root,
            &self.root,
            Some(&self.home),
            Some(&self.capture),
            Some(&self.core),
        )
        .expect("explicit private source-command environment")
    }

    fn boundary_negatives_before_any_command(&self) {
        let marker = self.root.join("command-must-not-be-constructed");
        for (home, capture, core) in [
            (
                None,
                Some(self.capture.as_path()),
                Some(self.core.as_path()),
            ),
            (Some(self.home.as_path()), None, Some(self.core.as_path())),
            (
                Some(self.home.as_path()),
                Some(self.capture.as_path()),
                None,
            ),
            (
                Some(self.root.as_path()),
                Some(self.capture.as_path()),
                Some(self.core.as_path()),
            ),
            (
                Some(self.home.as_path()),
                Some(self.capture.as_path()),
                Some(Path::new("/inert-foreign-cli")),
            ),
        ] {
            if isolation::checked(&self.root, &self.root, home, capture, core).is_ok() {
                fs::write(&marker, b"unsafe").unwrap();
            }
            assert!(
                !marker.exists(),
                "negative boundary reached a mutation marker"
            );
        }
        for name in [".claude", ".codex"] {
            let profile = self.home.join(name);
            fs::remove_dir(&profile).unwrap();
            symlink(self.root.join("escape"), &profile).unwrap();
            assert!(isolation::checked(
                &self.root,
                &self.root,
                Some(&self.home),
                Some(&self.capture),
                Some(&self.core)
            )
            .is_err());
            assert_eq!(fs::read_dir(self.root.join("escape")).unwrap().count(), 0);
            fs::remove_file(&profile).unwrap();
            fs::create_dir(&profile).unwrap();
            fs::set_permissions(profile, fs::Permissions::from_mode(0o700)).unwrap();
        }
        self.checked();
    }

    fn command(&self, program: &Path) -> assert_cmd::Command {
        let checked = self.checked(); // Always before even constructing Command.
        assert_eq!(checked.core, self.core);
        let mut command = service_boundary::command(program);
        command
            .env_clear()
            .current_dir(&self.repo)
            .env("HOME", checked.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("KEITO_CAPTURE_HOME", checked.capture)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("APPDATA", self.home.join("AppData"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("PATH", self.root.join("bin"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        command
    }

    fn cli(&self) -> assert_cmd::Command {
        self.command(&self.core)
    }

    fn source(&self, namespace: &str) -> PathBuf {
        if namespace == "claude-code" {
            self.home
                .join(format!(".claude/projects/synthetic/{SESSION}.jsonl"))
        } else {
            self.home.join(format!(
                ".codex/sessions/2026/11/08/rollout-synthetic-{SESSION}.jsonl"
            ))
        }
    }

    fn source_canaries(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut before = BTreeMap::new();
        for (namespace, _) in NAMESPACES {
            let source = self.source(namespace);
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(&source, b"PRIVATE_SOURCE_CONTENT_MUST_NOT_BE_RETAINED\n").unwrap();
            before.insert(
                source,
                b"PRIVATE_SOURCE_CONTENT_MUST_NOT_BE_RETAINED\n".to_vec(),
            );
        }
        before
    }

    fn assert_no_raw_leak(&self, output: &Output) {
        for bytes in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.contains("PRIVATE_"));
            assert!(!text.contains(self.root.to_str().unwrap()));
        }
        for table in ["sessions", "outbox"] {
            let database =
                rusqlite::Connection::open(self.capture.join("capture.sqlite3")).unwrap();
            let column = if table == "sessions" { "data" } else { "body" };
            let mut statement = database
                .prepare(&format!("SELECT {column} FROM {table}"))
                .unwrap();
            for row in statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
            {
                let text = row.unwrap();
                assert!(!text.contains("PRIVATE_") && !text.contains(self.root.to_str().unwrap()));
            }
        }
    }
}

#[test]
fn normal_usage_status_defaults_disabled_independently_for_both_namespaces() {
    let fixture = Fixture::new();
    let before = fixture.source_canaries();
    fixture
        .cli()
        .args(["capture", "status", "--json"])
        .assert()
        .success();
    for (namespace, _) in NAMESPACES {
        let output = fixture
            .cli()
            .args([
                "capture", "usage", "status", "--repo", "--agent", namespace, "--json",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "missing normal numeric-consent status for {namespace}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(status["enabled"], false);
        assert_eq!(status["status"], "UNAVAILABLE");
        assert_eq!(status["reason"], "NUMERIC_CAPTURE_DISABLED");
        assert_eq!(status["source_namespace"], namespace);
        fixture.assert_no_raw_leak(&output);
    }
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(
        !fixture.capture.join("profiles").exists(),
        "disabled status must not create a functional profile record"
    );
}

#[test]
fn numeric_enable_requires_separate_explicit_confirmation_before_any_profile_work() {
    let fixture = Fixture::new();
    for (namespace, _) in NAMESPACES {
        let output = fixture
            .cli()
            .args(["capture", "usage", "enable", "--repo", "--agent", namespace])
            .arg("--profile-home")
            .arg(&fixture.home)
            .arg("--harness-executable")
            .arg(fixture.root.join("inert-never-execute"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("--confirm"),
            "the missing independent consent must be visible"
        );
        assert!(!fixture.capture.join("profiles").exists());
        fixture.assert_no_raw_leak(&output);
    }
}

#[test]
fn normal_numeric_disable_revokes_only_selected_namespace_without_opening_a_profile() {
    let fixture = Fixture::new();
    let database = rusqlite::Connection::open(fixture.capture.join("capture.sqlite3")).unwrap();
    let binding: String = database
        .query_row("SELECT data FROM bindings", [], |row| row.get(0))
        .unwrap();
    let binding: Value = serde_json::from_str(&binding).unwrap();
    // These are path-free, deliberately unqualified policy rows. They cannot
    // provide the private profile, guard or origin proof needed for source reads.
    for (namespace, _) in NAMESPACES {
        let policy = json!({"policy_version":1,"binding_id":binding["id"],"binding_version":binding["version"],
            "source_namespace":namespace,"profile_id":capture::hash(format!("unqualified-{namespace}")),
            "profile_digest":capture::hash("no-functional-profile"),"epoch":10,"enabled":true});
        database
            .execute(
                "INSERT INTO usage_consents(binding_id,namespace,data) VALUES(?1,?2,?3)",
                rusqlite::params![
                    binding["id"].as_str().unwrap(),
                    namespace,
                    policy.to_string()
                ],
            )
            .unwrap();
    }
    for (namespace, _) in NAMESPACES {
        let other = if namespace == "claude-code" {
            "codex"
        } else {
            "claude-code"
        };
        let prior_other: String = database
            .query_row(
                "SELECT data FROM usage_consents WHERE namespace=?1",
                [other],
                |row| row.get(0),
            )
            .unwrap();
        let output = fixture
            .cli()
            .args([
                "capture", "usage", "disable", "--repo", "--agent", namespace, "--json",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "explicit local numeric withdrawal must work without a qualified runtime/profile"
        );
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(status["enabled"], false);
        assert_eq!(status["status"], "UNAVAILABLE");
        let chosen: String = database
            .query_row(
                "SELECT data FROM usage_consents WHERE namespace=?1",
                [namespace],
                |row| row.get(0),
            )
            .unwrap();
        let chosen: Value = serde_json::from_str(&chosen).unwrap();
        assert_eq!(chosen["enabled"], false);
        assert_eq!(chosen["epoch"], 11);
        let after_other: String = database
            .query_row(
                "SELECT data FROM usage_consents WHERE namespace=?1",
                [other],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            after_other, prior_other,
            "withdrawing one namespace rewrote the other consent"
        );
        fixture.assert_no_raw_leak(&output);
    }
    assert!(!fixture.capture.join("profiles").exists());
}

#[test]
fn normal_numeric_enable_requires_an_embedded_pair_before_profile_or_version_work() {
    let fixture = Fixture::new();
    for (namespace, _) in NAMESPACES {
        let output = fixture
            .cli()
            .args([
                "capture",
                "usage",
                "enable",
                "--repo",
                "--agent",
                namespace,
                "--confirm",
            ])
            .arg("--profile-home")
            .arg(&fixture.home)
            .arg("--harness-executable")
            .arg(fixture.root.join("never-executed-version-probe"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("intact official keito / keito-capture-guard release pair"));
        assert!(!fixture.capture.join("profiles").exists());
        let database = rusqlite::Connection::open(fixture.capture.join("capture.sqlite3")).unwrap();
        let rows: i64 = database
            .query_row("SELECT COUNT(*) FROM usage_consents", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        fixture.assert_no_raw_leak(&output);
    }
}

#[test]
fn default_normal_hooks_remain_metadata_only_even_with_supported_version_and_active_path() {
    let fixture = Fixture::new();
    let before = fixture.source_canaries();
    for (namespace, version) in NAMESPACES {
        for event in ["prompt", "stop"] {
            let input = json!({"session_id":SESSION,"turn_id":"turn_synthetic","prompt_id":"turn_synthetic","cwd":fixture.repo,"transcript_path":fixture.source(namespace),"prompt":"PRIVATE_PROMPT_CANARY","tool_input":{"secret":"PRIVATE_TOOL_CANARY"}});
            let output = fixture
                .cli()
                .args([
                    "capture",
                    "hook",
                    "--agent",
                    namespace,
                    "--event",
                    event,
                    "--harness-version",
                    version,
                ])
                .write_stdin(input.to_string())
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(output.stdout.is_empty());
            fixture.assert_no_raw_leak(&output);
        }
    }
    let database = rusqlite::Connection::open(fixture.capture.join("capture.sqlite3")).unwrap();
    let mut statement = database
        .prepare("SELECT body FROM outbox ORDER BY id")
        .unwrap();
    let bodies: Vec<Value> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect();
    assert_eq!(
        bodies.len(),
        2,
        "normal hooks must preserve both namespaces' timing path"
    );
    for body in bodies {
        assert_eq!(body["usage"]["status"], "UNAVAILABLE");
        assert!(body["usage"]
            .get("records")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty));
    }
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(!fixture.capture.join("profiles").exists());
}

// R12 regression: policies below are deliberately unqualified local fixtures.
// They permit status inspection, never construct a normal numeric capability.
fn r12_signal(
    store: &mut Store,
    repo: &capture::Repository,
    scope: (&str, &str, &str),
    boundary: (keito_cli::cli::capture::CaptureEvent, &str, i64),
) {
    let (source, version, session) = scope;
    let (event, turn, seconds) = boundary;
    store
        .event(
            repo,
            capture::Signal {
                source,
                resumed: false,
                source_id: session,
                turn_id: Some(turn),
                version,
                event,
                clock: &capture::Clock {
                    wall: 1_794_067_200 + seconds,
                    monotonic: 1000 + seconds,
                    boot: "r12-private-clock".into(),
                },
            },
        )
        .unwrap();
}

fn r12_policy(database: &rusqlite::Connection, binding: &Binding, namespace: &str) {
    let policy = json!({"policy_version":1,"binding_id":binding.id,"binding_version":binding.version,
        "source_namespace":namespace,"profile_id":capture::hash(format!("r12-unqualified-{namespace}")),
        "profile_digest":capture::hash("r12-no-functional-profile"),"epoch":10,"enabled":true});
    database
        .execute(
            "INSERT INTO usage_consents(binding_id,namespace,data) VALUES(?1,?2,?3)",
            rusqlite::params![binding.id, namespace, policy.to_string()],
        )
        .unwrap();
}

fn r12_other_snapshot(database: &rusqlite::Connection, binding: &Binding) -> Vec<String> {
    let mut output = vec![database
        .query_row(
            "SELECT data FROM bindings WHERE repo_key=?1",
            [&binding.repository_key],
            |row| row.get::<_, String>(0),
        )
        .unwrap()];
    for sql in [
        "SELECT namespace || data FROM usage_consents WHERE binding_id=?1 ORDER BY namespace",
        "SELECT id || data FROM sessions WHERE binding_id=?1 ORDER BY id",
        "SELECT json_object('id',id,'body',body,'hash',payload_hash,'state',state,'reason',reason,'claim',claimed_until) FROM outbox WHERE session_id IN(SELECT id FROM sessions WHERE binding_id=?1) ORDER BY id",
    ] {
        let mut statement = database.prepare(sql).unwrap();
        output.extend(statement.query_map([&binding.id], |row| row.get::<_, String>(0)).unwrap().map(Result::unwrap));
    }
    output
}

fn r12_freeze_numeric_fixture(
    database: &rusqlite::Connection,
    binding: &Binding,
    source: &str,
    source_id: &str,
) -> (String, String, String) {
    let session_id: String = database.query_row("SELECT id FROM sessions WHERE binding_id=?1 AND source_id=?2 AND json_extract(data,'$.source_namespace')=?3",
        rusqlite::params![binding.id, source_id, source], |row| row.get(0)).unwrap();
    let body: String = database
        .query_row(
            "SELECT body FROM outbox WHERE session_id=?1",
            [&session_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut body: Value = serde_json::from_str(&body).unwrap();
    let record = json!({"source_adapter":if source=="codex" {"CODEX_ROLLOUT_0_167_0_V1"} else {"CLAUDE_JSONL_2_1_319_V1"},
        "source_record_id":"r12_frozen_request","source_turn_id":if source=="codex" {Some("frozen_turn")} else {None},
        "model":null,"model_provenance":"UNKNOWN","counter_semantics":"REQUEST_DELTA",
        "input_basis":if source=="codex" {"CODEX_TOTAL_INPUT"} else {"CLAUDE_UNCACHED_INPUT"},
        "input_tokens":7,"cached_input_tokens":null,"cache_write_input_tokens":null,"output_tokens":3,"reasoning_output_tokens":null});
    body["usage"] = json!({"schema_version":1,"status":"PARTIAL","reason":"OBSERVED_REQUEST_RECORDS","records":[record.clone()]});
    let body = body.to_string();
    let digest = capture::hash(&body);
    database
        .execute(
            "UPDATE outbox SET body=?2,payload_hash=?3 WHERE session_id=?1",
            rusqlite::params![session_id, body, digest],
        )
        .unwrap();
    let ledger = json!({"records":[record],"record_revisions":[1],"queued":1,"incomplete":false,"held":false,
        "consent_epoch":0,"baseline_epoch":0,"excluded_request_ids":[]});
    database
        .execute(
            "UPDATE sessions SET data=json_set(data,'$.usage_ledger',json(?2)) WHERE id=?1",
            rusqlite::params![session_id, ledger.to_string()],
        )
        .unwrap();
    (session_id, body, digest)
}

#[test]
fn normal_usage_status_persists_changed_repository_revocation_after_exact_restoration() {
    use keito_cli::cli::capture::CaptureEvent::{Prompt, Stop};
    let mut failures = Vec::new();
    let mut observations = Vec::new();
    for (selected, selected_version) in NAMESPACES {
        for mutation in ["hint", "remote", "disabled"] {
            let fixture = Fixture::new();
            // Every case uses TempDir cleanup; aggregate failure facts stay in the test log.
            let canaries = fixture.source_canaries();
            let database =
                rusqlite::Connection::open(fixture.capture.join("capture.sqlite3")).unwrap();
            let key = capture::hash(fixture.repo.join(".git").to_string_lossy().as_bytes());
            let mut store = Store::open(&fixture.capture).unwrap();
            let binding = store.binding(&key).unwrap().unwrap();
            let repo = capture::Repository {
                key: binding.repository_key.clone(),
                remote_hash: binding.remote_hash.clone(),
                hint_hash: binding.hint_hash.clone(),
                linked_without_hint: false,
                disabled: false,
            };
            let mut other = binding.clone();
            other.repository_key = capture::hash("r12-unrelated-repository");
            other.id = String::new();
            other.version = 0;
            let other = store.approve(other).unwrap();
            let other_repo = capture::Repository {
                key: other.repository_key.clone(),
                remote_hash: other.remote_hash.clone(),
                hint_hash: other.hint_hash.clone(),
                linked_without_hint: false,
                disabled: false,
            };
            let mut frozen = Vec::new();
            for (namespace, version) in NAMESPACES {
                r12_signal(
                    &mut store,
                    &repo,
                    (namespace, version, "11111111-1111-4111-8111-111111111111"),
                    (Prompt, "frozen_turn", 0),
                );
                r12_signal(
                    &mut store,
                    &repo,
                    (namespace, version, "11111111-1111-4111-8111-111111111111"),
                    (Stop, "frozen_turn", 10),
                );
                frozen.push(r12_freeze_numeric_fixture(
                    &database,
                    &binding,
                    namespace,
                    "11111111-1111-4111-8111-111111111111",
                ));
                r12_signal(
                    &mut store,
                    &repo,
                    (namespace, version, SESSION),
                    (Prompt, "active_turn", 20),
                );
                r12_signal(
                    &mut store,
                    &other_repo,
                    (namespace, version, "33333333-3333-4333-8333-333333333333"),
                    (Prompt, "other_turn", 0),
                );
                r12_signal(
                    &mut store,
                    &other_repo,
                    (namespace, version, "33333333-3333-4333-8333-333333333333"),
                    (Stop, "other_turn", 5),
                );
                r12_signal(
                    &mut store,
                    &other_repo,
                    (namespace, version, "44444444-4444-4444-8444-444444444444"),
                    (Prompt, "other_active", 10),
                );
                r12_policy(&database, &binding, namespace);
                r12_policy(&database, &other, namespace);
            }
            drop(store);
            let unrelated_before = r12_other_snapshot(&database, &other);
            let original = fixture
                .cli()
                .args([
                    "capture", "usage", "status", "--repo", "--agent", selected, "--json",
                ])
                .output()
                .unwrap();
            assert!(original.status.success());
            assert_eq!(
                serde_json::from_slice::<Value>(&original.stdout).unwrap()["enabled"],
                true
            );
            if mutation == "remote" {
                let result = fixture
                    .command(Path::new("/usr/bin/git"))
                    .args([
                        "remote",
                        "add",
                        "origin",
                        "https://example.invalid/r12-changed",
                    ])
                    .output()
                    .unwrap();
                assert!(result.status.success());
            } else {
                fs::create_dir_all(fixture.repo.join(".keito")).unwrap();
                fs::write(
                    fixture.repo.join(".keito/config.yml"),
                    if mutation == "disabled" {
                        "agent_tracking:\n  enabled: false\n"
                    } else {
                        "project_id: project_r12_changed\n"
                    },
                )
                .unwrap();
            }
            let changed = fixture
                .cli()
                .args([
                    "capture", "usage", "status", "--repo", "--agent", selected, "--json",
                ])
                .output()
                .unwrap();
            assert!(changed.status.success());
            let changed: Value = serde_json::from_slice(&changed.stdout).unwrap();
            let observed_raw: String = database
                .query_row(
                    "SELECT data FROM bindings WHERE repo_key=?1",
                    [&binding.repository_key],
                    |row| row.get(0),
                )
                .unwrap();
            let observed: Value = serde_json::from_str(&observed_raw).unwrap();
            let active_after_observe: i64=database.query_row("SELECT COUNT(*) FROM sessions WHERE binding_id=?1 AND json_type(data,'$.active')='object'",[&binding.id],|row|row.get(0)).unwrap();
            if mutation == "remote" {
                let result = fixture
                    .command(Path::new("/usr/bin/git"))
                    .args(["remote", "remove", "origin"])
                    .output()
                    .unwrap();
                assert!(result.status.success());
            } else {
                fs::remove_file(fixture.repo.join(".keito/config.yml")).unwrap();
            }
            let restored = fixture
                .cli()
                .args([
                    "capture", "usage", "status", "--repo", "--agent", selected, "--json",
                ])
                .output()
                .unwrap();
            assert!(restored.status.success());
            fixture.assert_no_raw_leak(&restored);
            let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
            let mut case_failures = Vec::new();
            for (okay, label) in [
                (changed["enabled"] == false, "changed-disabled"),
                (
                    observed["revoked"] == true && observed["paused"] == true,
                    "durable-binding",
                ),
                (active_after_observe == 0, "clock-abandoned"),
                (restored["enabled"] == false, "restored-stays-disabled"),
            ] {
                if !okay {
                    case_failures.push(label);
                }
            }
            for (namespace, _) in NAMESPACES {
                let raw: String = database
                    .query_row(
                        "SELECT data FROM usage_consents WHERE binding_id=?1 AND namespace=?2",
                        rusqlite::params![binding.id, namespace],
                        |row| row.get(0),
                    )
                    .unwrap();
                let policy: Value = serde_json::from_str(&raw).unwrap();
                if policy["enabled"] != false || policy["epoch"] != 11 {
                    case_failures.push("both-binding-policies-revoked");
                }
                let raw:String=database.query_row("SELECT data FROM sessions WHERE binding_id=?1 AND source_id=?2 AND json_extract(data,'$.source_namespace')=?3",rusqlite::params![binding.id,SESSION,namespace],|row|row.get(0)).unwrap();
                let session: Value = serde_json::from_str(&raw).unwrap();
                if !session["active"].is_null()
                    || !session["segments"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|segment| {
                            segment["duration_seconds"] == 0
                                && segment["completeness"] == "INCOMPLETE"
                        })
                {
                    case_failures.push("observed-clock-zero-incomplete");
                }
                if !session["usage_ledger"]["baseline_epoch"].is_null()
                    || session["usage_ledger"]["consent_epoch"]
                        .as_u64()
                        .unwrap_or(0)
                        == 0
                {
                    case_failures.push("fresh-baseline-epoch-required");
                }
            }
            for (session_id, body, digest) in &frozen {
                let after: (String, String, String) = database
                    .query_row(
                        "SELECT body,payload_hash,state FROM outbox WHERE session_id=?1",
                        [session_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .unwrap();
                if after.0 != *body || after.1 != *digest || after.2 != "held" {
                    case_failures.push("frozen-body-held-unchanged");
                }
            }
            for (event, turn) in [("stop", "active_turn"), ("prompt", "new_turn")] {
                let input = json!({"session_id":SESSION,"turn_id":turn,"prompt_id":turn,"cwd":fixture.repo,"transcript_path":fixture.source(selected)});
                let output = fixture
                    .cli()
                    .args([
                        "capture",
                        "hook",
                        "--agent",
                        selected,
                        "--event",
                        event,
                        "--harness-version",
                        selected_version,
                    ])
                    .write_stdin(input.to_string())
                    .output()
                    .unwrap();
                assert!(output.status.success());
                fixture.assert_no_raw_leak(&output);
            }
            let new_active:i64=database.query_row("SELECT COUNT(*) FROM sessions WHERE binding_id=?1 AND json_type(data,'$.active')='object'",[&binding.id],|row|row.get(0)).unwrap();
            if new_active != 0 {
                case_failures.push("restored-cannot-resume-or-start-clock");
            }
            if r12_other_snapshot(&database, &other) != unrelated_before {
                case_failures.push("unrelated-repository-unchanged");
            }
            for (path, bytes) in canaries {
                assert_eq!(fs::read(path).unwrap(), bytes);
            }
            assert!(!fixture.capture.join("profiles").exists());
            observations.push(json!({"namespace":selected,"mutation":mutation,"fixture_cleanup":"TempDir","failures":case_failures}));
            failures.extend(
                case_failures
                    .iter()
                    .map(|failure| format!("{selected}/{mutation}/{failure}")),
            );
        }
    }
    eprintln!("R12_STATUS_FIRST_OBSERVATIONS {}", json!(observations));
    assert!(
        failures.is_empty(),
        "R12 durable usage-status restoration failed: {}",
        failures.join(", ")
    );
}
