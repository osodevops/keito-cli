//! Broken cores must reach the guardian before any accounting continuation.
//! Owned faults are injected after private preflight and command construction.
#![cfg(unix)]
#[path = "support/mutation_environment.rs"]
mod isolation;
use keito_cli::{
    capture::{self, Binding, Clock, Repository, Signal, Store},
    cli::capture::CaptureEvent,
};
use std::{
    fs,
    io::Write,
    os::unix::{
        fs::{symlink, PermissionsExt},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
struct Case {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    capture: PathBuf,
    core: PathBuf,
    guard: PathBuf,
    repo: Repository,
    store: Store,
}
impl Case {
    fn new(name: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let owner = temp.path().canonicalize().unwrap();
        let root = owner.join("case");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let capture = home.join("capture");
        fs::create_dir_all(&capture).unwrap();
        fs::set_permissions(&capture, fs::Permissions::from_mode(0o700)).unwrap();
        let core = root.join("core");
        let guard = root.join("guard");
        fs::write(
            &core,
            "#!/bin/sh\n: > \"$KEITO_DISPATCH_TEST_ENTERED\"\nexit 89\n",
        )
        .unwrap();
        fs::set_permissions(&core, fs::Permissions::from_mode(0o700)).unwrap();
        fs::copy(assert_cmd::cargo::cargo_bin!("keito-capture-guard"), &guard).unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("sleep"), "#!/bin/sh\nexec /bin/sleep \"$@\"\n").unwrap();
        fs::set_permissions(bin.join("sleep"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(bin.join("bash"), "#!/bin/sh\nexec /bin/bash \"$@\"\n").unwrap();
        fs::set_permissions(bin.join("bash"), fs::Permissions::from_mode(0o700)).unwrap();
        let repo = Repository {
            key: capture::hash(name),
            remote_hash: capture::hash("remote"),
            hint_hash: capture::hash("hint"),
            disabled: false,
            linked_without_hint: false,
        };
        let mut store = Store::open(&capture).unwrap();
        store
            .approve(Binding {
                repository_key: repo.key.clone(),
                remote_hash: repo.remote_hash.clone(),
                hint_hash: repo.hint_hash.clone(),
                id: String::new(),
                version: 0,
                workspace_id: "co_fixture".into(),
                owner_id: "owner".into(),
                origin_hash: capture::hash("synthetic"),
                project_id: "project".into(),
                task_id: "task".into(),
                agents: vec!["codex".into(), "claude-code".into()],
                paused: false,
                revoked: false,
            })
            .unwrap();
        Self {
            _temp: temp,
            root,
            home,
            capture,
            core,
            guard,
            repo,
            store,
        }
    }
    fn event(&mut self, source: &str, event: CaptureEvent, second: i64) {
        self.store
            .event(
                &self.repo,
                Signal {
                    source,
                    resumed: false,
                    source_id: "session",
                    turn_id: Some("turn"),
                    version: if source == "codex" {
                        "0.167.0"
                    } else {
                        "2.1.319"
                    },
                    event,
                    clock: &Clock {
                        wall: 1_794_067_200 + second,
                        monotonic: 100_000 + second,
                        boot: "fixture-boot".into(),
                    },
                },
            )
            .unwrap();
    }
    fn command(&self, program: &Path) -> Command {
        let checked = isolation::checked(
            self.root.parent().unwrap(),
            &self.root,
            Some(&self.home),
            Some(&self.capture),
            Some(&self.core),
        )
        .unwrap();
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .current_dir(&self.root)
            .env("HOME", &checked.home)
            .env("KEITO_CAPTURE_HOME", &checked.capture)
            .env("CODEX_HOME", checked.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", checked.home.join(".claude"))
            .env("XDG_CONFIG_HOME", checked.home.join("config"))
            .env("APPDATA", checked.home.join("AppData"))
            .env("PATH", self.root.join("bin"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("KEITO_CLI_BIN", &checked.core)
            .env("KEITO_CAPTURE_GUARD_BIN", &self.guard)
            .env(
                "KEITO_CAPTURE_CORE_SHA256",
                capture::hash(fs::read(&checked.core).unwrap()),
            )
            .env(
                "KEITO_DISPATCH_TEST_ENTERED",
                self.root.join("MUST_NOT_ENTER_CORE"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
    fn flags(&self) -> usize {
        fs::read_dir(self.capture.join("capture-invocations"))
            .map(|v| v.count())
            .unwrap_or(0)
    }
    fn scan(&self) {
        fn visit(path: &Path) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                let m = fs::symlink_metadata(&path).unwrap();
                assert!(!m.file_type().is_symlink());
                if m.is_dir() {
                    visit(&path);
                } else {
                    let bytes = fs::read(path).unwrap();
                    assert!(!bytes
                        .windows(b"PRIVATE_DISPATCH_CANARY".len())
                        .any(|v| v == b"PRIVATE_DISPATCH_CANARY"));
                }
            }
        }
        visit(&self.capture);
    }
}
fn refused_core(route: &str, mode: &str) {
    let mut case = Case::new(&format!("{route}-{mode}"));
    let source = if route == "plugin" {
        "claude-code"
    } else {
        "codex"
    };
    case.event(source, CaptureEvent::Prompt, 0);
    let mut cmd = case.command(if route == "direct" {
        &case.guard
    } else {
        Path::new("/bin/bash")
    });
    match route {
        "direct" => {
            cmd.arg("--core").arg(&case.core).args([
                "--agent",
                "codex",
                "--event",
                "stop",
                "--harness-version",
                "0.167.0",
            ]);
        }
        "plugin" => {
            let plugin: serde_json::Value = serde_json::from_str(include_str!(
                "../skills/keito-time-track/hooks/claude-plugin.json"
            ))
            .unwrap();
            let literal = plugin["hooks"]["Stop"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            cmd.env(
                "CLAUDE_PLUGIN_ROOT",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills"),
            )
            .env("KEITO_HARNESS_VERSION", "2.1.319")
            .args(["-c", literal]);
        }
        "bundled" => {
            cmd.arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/skills/keito-time-track/hooks/dispatch.sh"
            ))
            .args([
                "--agent",
                "codex",
                "--event",
                "stop",
                "--harness-version",
                "0.167.0",
            ]);
        }
        _ => unreachable!(),
    }
    let saved = case.root.join("saved-core");
    match mode {
        "missing" => fs::rename(&case.core, &saved).unwrap(),
        "mode" => fs::set_permissions(&case.core, fs::Permissions::from_mode(0o600)).unwrap(),
        "symlink" => {
            fs::rename(&case.core, &saved).unwrap();
            symlink(&saved, &case.core).unwrap();
        }
        _ => unreachable!(),
    }
    // A deliberate owned fault after checked() must exercise refusal, never a
    // foreign path or an inherited operator environment.
    let start = Instant::now();
    let mut child = cmd.spawn().unwrap();
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"prompt\":\"PRIVATE_DISPATCH_CANARY\"}");
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success() && out.stdout.is_empty() && out.stderr.is_empty(),
        "route={route} mode={mode}: exit={:?} signal={:?} stdout_bytes={} stderr_bytes={} elapsed_ms={:.3} shell_termination_notice={} shell_kill_notice={}",
        out.status.code(), out.status.signal(), out.stdout.len(), out.stderr.len(),
        start.elapsed().as_secs_f64() * 1000.0,
        out.stderr.windows(b"Terminated".len()).any(|bytes| bytes == b"Terminated"),
        out.stderr.windows(b"Killed".len()).any(|bytes| bytes == b"Killed")
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    let flags = case.flags();
    assert!(!case.root.join("MUST_NOT_ENTER_CORE").exists());
    match mode {
        "missing" => fs::rename(&saved, &case.core).unwrap(),
        "mode" => fs::set_permissions(&case.core, fs::Permissions::from_mode(0o700)).unwrap(),
        "symlink" => {
            fs::remove_file(&case.core).unwrap();
            fs::rename(&saved, &case.core).unwrap();
        }
        _ => unreachable!(),
    }
    case.event(source, CaptureEvent::Stop, 150);
    let session = case.store.preview("session").unwrap().remove(0);
    let seconds: i64 = session
        .segments
        .iter()
        .filter(|v| v.completeness == "COMPLETE")
        .map(|v| v.duration_seconds)
        .sum();
    case.scan();
    assert_eq!(flags, 1, "route={route} mode={mode}: no durable refusal");
    assert_eq!(seconds, 0, "missed Stop became COMPLETE time");
    assert!(session.needs_attention);
}
#[test]
fn bundled_missing_core() {
    refused_core("bundled", "missing");
}
#[test]
fn bundled_nonexecutable_core() {
    refused_core("bundled", "mode");
}
#[test]
fn bundled_symlink_core() {
    refused_core("bundled", "symlink");
}
#[test]
fn plugin_missing_core() {
    refused_core("plugin", "missing");
}
#[test]
fn plugin_nonexecutable_core() {
    refused_core("plugin", "mode");
}
#[test]
fn plugin_symlink_core() {
    refused_core("plugin", "symlink");
}
#[test]
fn direct_guard_missing_core() {
    refused_core("direct", "missing");
}
#[test]
fn direct_guard_nonexecutable_core() {
    refused_core("direct", "mode");
}
#[test]
fn direct_guard_symlink_core() {
    refused_core("direct", "symlink");
}
