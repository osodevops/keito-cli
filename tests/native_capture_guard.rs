//! Native guard acceptance. Every process, even a fail-before-write case, must
//! receive explicit temporary HOME/capture roots and a run-owned core path.
#![cfg(unix)]
#[path = "support/mutation_environment.rs"]
mod isolation;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};
struct Run {
    _temp: tempfile::TempDir,
    root: PathBuf,
    core: PathBuf,
}
fn run() -> &'static Run {
    static RUN: OnceLock<Run> = OnceLock::new();
    RUN.get_or_init(||{
 let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let source=root.join("fake-core.rs");let core=root.join("fake-core");
 fs::write(&source,r#"use std::{fs::{self,File,OpenOptions},io::{Read,Write},os::fd::FromRawFd,time::Duration};
unsafe extern "C"{fn fcntl(fd:i32,cmd:i32,...)->i32;}
fn main(){let image_fd=std::env::var("KEITO_CAPTURE_IMAGE_FD").unwrap().parse::<i32>().unwrap();for fd in 4..256{if fd!=image_fd{assert_eq!(unsafe{fcntl(fd,1)},-1,"unexpected inherited descriptor {fd}");}}let image=unsafe{File::from_raw_fd(image_fd)};assert!(image.metadata().unwrap().is_file());drop(image);assert_eq!(unsafe{fcntl(image_fd,1)},-1,"image descriptor must close before core parsing");let mode=std::env::var("NATIVE_TEST_CORE_MODE").unwrap();let home=std::path::PathBuf::from(std::env::var_os("KEITO_CAPTURE_HOME").unwrap());let nonce=std::env::var("KEITO_CAPTURE_INVOCATION_NONCE").unwrap();let flag=home.join("capture-invocations").join(format!("{nonce}.pending"));assert!(flag.exists(),"core entered before durable flag");if mode=="stall"{std::thread::sleep(Duration::from_secs(5));return;}let mut bytes=Vec::new();std::io::stdin().read_to_end(&mut bytes).unwrap();if let Some(path)=std::env::var_os("NATIVE_TEST_INPUT_LOG"){fs::write(path,bytes).unwrap();}if mode=="edit"{let mut file=OpenOptions::new().write(true).truncate(true).open(&flag).unwrap();file.write_all(b"UNEXPECTED_SAME_INODE_EDIT").unwrap();file.sync_all().unwrap();}if mode!="missing"{let fd=std::env::var("KEITO_CAPTURE_ACK_FD").unwrap().parse::<i32>().unwrap();let mut file=unsafe{File::from_raw_fd(fd)};file.write_all(b"KEITO_CAPTURE_DURABLE_V1\n").unwrap();if mode=="extra"{file.write_all(b"extra").unwrap();}}if mode=="ack-stall"{std::thread::sleep(Duration::from_secs(5));}if mode=="nonzero"{std::process::exit(23);}}
"#).unwrap();assert!(Command::new("rustc").args(["--edition=2021","-C","opt-level=s","-C","strip=symbols"]).arg(&source).arg("-o").arg(&core).status().unwrap().success());Run{_temp:temp,root,core}
})
}
struct Scope {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    capture: PathBuf,
}
impl Scope {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("case-")
            .tempdir_in(&run().root)
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        let capture = home.join("capture");
        fs::create_dir(&capture).unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        let poison = bin.join("keito");
        fs::write(
            &poison,
            "#!/bin/sh\nprintf forbidden > REAL_CLI_FALLBACK_CALLED\nexit 89\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&capture, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(poison, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _temp: temp,
            root,
            home,
            capture,
        }
    }
    fn command(&self, core: &Path, mode: &str) -> Command {
        let environment = isolation::checked(
            &run().root,
            &self.root,
            Some(&self.home),
            Some(&self.capture),
            Some(core),
        )
        .expect("refusing non-isolated mutation command");
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("keito-capture-guard"));
        cmd.env_clear()
            .current_dir(&self.root)
            .env("PATH", self.root.join("bin"))
            .env("HOME", &environment.home)
            .env("CODEX_HOME", environment.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", environment.home.join(".claude"))
            .env("KEITO_CAPTURE_HOME", &environment.capture)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env_remove("KEITO_CLI_BIN")
            .env_remove("KEITO_CAPTURE_INVOCATION_NONCE")
            .env_remove("KEITO_CAPTURE_ACK_FD")
            .env(
                "KEITO_CAPTURE_CORE_SHA256",
                keito_cli::capture::hash(fs::read(&run().core).unwrap()),
            )
            .env("NATIVE_TEST_CORE_MODE", mode)
            .env_remove("KEITO_API_KEY")
            .env_remove("KEITO_ACCOUNT_ID")
            .args(["--core"])
            .arg(&environment.core)
            .args([
                "--agent",
                "codex",
                "--event",
                "stop",
                "--harness-version",
                "0.167.0",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
    fn invoke(&self, mode: &str) -> std::process::Output {
        let mut child = self.command(&run().core, mode).spawn().unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(b"PRIVATE_NATIVE_HOOK_CANARY");
        let output = child.wait_with_output().unwrap();
        assert!(!self.root.join("REAL_CLI_FALLBACK_CALLED").exists());
        output
    }
    fn flags(&self) -> Vec<PathBuf> {
        match fs::read_dir(self.capture.join("capture-invocations")) {
            Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(error) => panic!("fixture capture inventory failed: {error}"),
        }
    }
}
#[test]
fn native_guard_publishes_before_core_and_cleans_only_exact_zero_exit_ack() {
    for mode in ["ack", "missing", "extra", "nonzero", "edit"] {
        let scope = Scope::new();
        let log = scope.root.join("input.log");
        let mut command = scope.command(&run().core, mode);
        command.env("NATIVE_TEST_INPUT_LOG", &log);
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"PRIVATE_NATIVE_HOOK_CANARY")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(log).unwrap(), b"PRIVATE_NATIVE_HOOK_CANARY");
        assert_eq!(scope.flags().len(), usize::from(mode != "ack"), "{mode}");
    }
}
#[test]
fn native_guard_never_entered_core_deadline_retains_durable_unknown() {
    let scope = Scope::new();
    let started = Instant::now();
    let output = scope.invoke("stall");
    assert!(output.status.success());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(scope.flags().len(), 1);
    assert_eq!(
        fs::read(&scope.flags()[0]).unwrap(),
        b"KEITO_CAPTURE_PENDING_V1\n"
    );
}
#[test]
fn native_guard_parallel_flags_are_distinct_and_visible_within_existing_budget() {
    let scope = Scope::new();
    let mut children = Vec::new();
    for _ in 0..2 {
        let mut child = scope.command(&run().core, "stall").spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"PRIVATE_NATIVE_HOOK_CANARY")
            .unwrap();
        children.push(child);
    }
    let started = Instant::now();
    loop {
        let pending = scope.capture.join("capture-invocations");
        let count = fs::read_dir(&pending)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".pending"))
                    .count()
            })
            .unwrap_or(0);
        if count == 2 {
            break;
        }
        if started.elapsed() >= Duration::from_millis(500) {
            let flags = scope.flags();
            for child in children {
                let output = child.wait_with_output().unwrap();
                eprintln!("guard stderr={}", String::from_utf8_lossy(&output.stderr));
            }
            panic!("two ordinary pending flags must appear without warming; flags={flags:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for child in children {
        assert!(child.wait_with_output().unwrap().status.success());
    }
    assert_eq!(scope.flags().len(), 2);
}
#[test]
fn missing_run_owned_core_never_falls_back_to_a_real_cli() {
    let scope = Scope::new();
    let missing = scope.root.join("missing-core");
    let output = scope.command(&missing, "missing").output().unwrap();
    assert!(output.status.success());
    assert_eq!(scope.flags().len(), 1);
}

#[test]
fn an_ack_without_child_exit_never_clears_uncertainty() {
    let scope = Scope::new();
    let started = Instant::now();
    assert!(scope.invoke("ack-stall").status.success());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(scope.flags().len(), 1);
}
#[test]
fn invocation_capacity_publishes_global_refusal_without_core_launch_or_eviction() {
    use std::os::unix::fs::PermissionsExt;
    let scope = Scope::new();
    let flags = scope.capture.join("capture-invocations");
    fs::create_dir(&flags).unwrap();
    fs::set_permissions(&flags, fs::Permissions::from_mode(0o700)).unwrap();
    for i in 0..1024 {
        let path = flags.join(format!("{i:064x}.pending"));
        fs::write(&path, b"KEITO_CAPTURE_PENDING_V1\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let log = scope.root.join("MUST_NOT_ENTER_CORE");
    let mut command = scope.command(&run().core, "ack");
    command.env("NATIVE_TEST_INPUT_LOG", &log);
    assert!(command.output().unwrap().status.success());
    assert!(!log.exists());
    assert_eq!(scope.flags().len(), 1024);
    let refusals: Vec<_> = fs::read_dir(&scope.capture)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("capture-refusal-")
        })
        .collect();
    assert_eq!(refusals.len(), 1);
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(refusals[0].path()).unwrap()).unwrap();
    assert_eq!(value["reason"], "QUEUE_LIMIT_REACHED");
    assert_eq!(value.as_object().unwrap().len(), 3);
}

#[test]
fn native_guard_timeout_handoff_reconciles_prior_open_turn_as_incomplete() {
    use keito_cli::capture::{self, Binding, Clock, Repository, Signal, Store};
    use keito_cli::cli::capture::CaptureEvent::{Prompt, Stop};
    let scope = Scope::new();
    let repo = Repository {
        key: capture::hash("native-handoff-repo"),
        remote_hash: capture::hash("remote"),
        hint_hash: capture::hash("hint"),
        linked_without_hint: false,
        disabled: false,
    };
    let mut store = Store::open(&scope.capture).unwrap();
    store
        .approve(Binding {
            repository_key: repo.key.clone(),
            remote_hash: repo.remote_hash.clone(),
            hint_hash: repo.hint_hash.clone(),
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
    let mut opened = Clock::now().unwrap();
    opened.wall -= 90;
    opened.monotonic -= 90;
    store
        .event(
            &repo,
            Signal {
                source: "codex",
                resumed: false,
                source_id: "native",
                turn_id: Some("turn"),
                version: "0.167.0",
                event: Prompt,
                clock: &opened,
            },
        )
        .unwrap();
    assert!(scope.invoke("stall").status.success());
    assert_eq!(scope.flags().len(), 1);
    store
        .event(
            &repo,
            Signal {
                source: "codex",
                resumed: false,
                source_id: "native",
                turn_id: Some("turn"),
                version: "0.167.0",
                event: Stop,
                clock: &Clock::now().unwrap(),
            },
        )
        .unwrap();
    let session = &store.preview("native").unwrap()[0];
    assert_eq!(session.segments[0].duration_seconds, 0);
    assert_eq!(session.segments[0].completeness, "INCOMPLETE");
    assert!(session.needs_attention);
    assert!(store.status().unwrap().storage_paused);
}
