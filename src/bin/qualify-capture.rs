//! Development-only paired-artifact acceptance. No harness, credentials,
//! network service, installed profile or real CLI fallback is used.
#[cfg(unix)]
#[path = "../../tests/support/mutation_environment.rs"]
mod isolation;
#[cfg(unix)]
mod qualification {
    use keito_cli::capture::{self, Binding, Store};
    use serde_json::{json, Value};
    use std::{
        fs,
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                fs::{MetadataExt, PermissionsExt},
                process::CommandExt,
            },
        },
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    const PENDING: &[u8] = b"KEITO_CAPTURE_PENDING_V1\n";
    #[derive(serde::Serialize)]
    struct Observation {
        elapsed_ms: f64,
        success: bool,
        exit_code: Option<i32>,
        stdout_bytes: usize,
        stderr_bytes: usize,
        spawn_error: Option<i32>,
        flags: usize,
    }
    struct Case {
        root: PathBuf,
        home: PathBuf,
        capture: PathBuf,
        core: PathBuf,
        guard: PathBuf,
        bin: PathBuf,
        repo: PathBuf,
        core_hash: String,
    }
    impl Case {
        fn new(source: &Path, run: &Path, name: &str) -> Self {
            let root = run.join(name);
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let home = root.join("home");
            fs::create_dir(&home).unwrap();
            let capture = home.join("capture");
            fs::create_dir(&capture).unwrap();
            fs::set_permissions(&capture, fs::Permissions::from_mode(0o700)).unwrap();
            let artifact = root.join("artifact");
            fs::create_dir(&artifact).unwrap();
            for name in [
                "keito",
                "keito-capture-guard",
                "capture-runtime-manifest.json",
            ] {
                let from = source.join(name);
                let m = fs::symlink_metadata(&from).unwrap();
                assert!(m.is_file() && !m.file_type().is_symlink());
                fs::copy(from, artifact.join(name)).unwrap();
            }
            let core = artifact.join("keito");
            let guard = artifact.join("keito-capture-guard");
            let manifest: Value = serde_json::from_slice(
                &fs::read(artifact.join("capture-runtime-manifest.json")).unwrap(),
            )
            .unwrap();
            let core_hash = manifest["files"]["keito"].as_str().unwrap().to_owned();
            assert_eq!(core_hash.len(), 64);
            let bin = root.join("bin");
            fs::create_dir(&bin).unwrap();
            fs::write(bin.join("keito"), "#!/bin/sh\nexit 89\n").unwrap();
            fs::set_permissions(bin.join("keito"), fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(
                root.join("dispatch.sh"),
                include_str!("../../skills/keito-time-track/hooks/dispatch.sh"),
            )
            .unwrap();
            fs::set_permissions(root.join("dispatch.sh"), fs::Permissions::from_mode(0o700))
                .unwrap();
            let repo = root.join("repo");
            fs::create_dir(&repo).unwrap();
            let result = Self {
                root,
                home,
                capture,
                core,
                guard,
                bin,
                repo,
                core_hash,
            };
            // Preserve the caller's real executable selection; private PATH
            // aliases remove fixture wrapper overhead without choosing a
            // different/faster Git implementation than the installed hook.
            let selected_git = std::env::var_os("PATH")
                .and_then(|paths| {
                    std::env::split_paths(&paths)
                        .map(|p| p.join("git"))
                        .find(|p| {
                            fs::metadata(p).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
                        })
                })
                .expect(
                    "Caller Git is unavailable; fixture setup failed before artifact execution",
                );
            let platform_git = selected_git.canonicalize().unwrap();
            let mut tools = serde_json::Map::new();
            for (name, path) in [
                ("git", platform_git),
                ("sleep", PathBuf::from("/bin/sleep")),
                ("bash", PathBuf::from("/bin/bash")),
            ] {
                let path = path.canonicalize().unwrap();
                let metadata = fs::metadata(&path).unwrap();
                assert!(
                    metadata.is_file()
                        && metadata.mode() & 0o111 != 0
                        && metadata.mode() & 0o022 == 0
                        && (metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() })
                );
                std::os::unix::fs::symlink(&path, result.bin.join(name)).unwrap();
                tools.insert(
                    name.into(),
                    json!({"path":path,"sha256":capture::hash(fs::read(&path).unwrap()),
                        "selection":if name=="git" {"caller_PATH"} else {"explicit_OS_utility"}}),
                );
            }
            tools.insert("selected_git_path".into(), json!(selected_git));
            fs::write(
                result.root.join("platform-tools.json"),
                serde_json::to_vec_pretty(&tools).unwrap(),
            )
            .unwrap();
            let template = result.root.join("git-template");
            fs::create_dir(&template).unwrap();
            let mut git = result.command(&result.bin.join("git"));
            assert!(git
                .arg("-C")
                .arg(&result.repo)
                .arg("init")
                .arg(format!("--template={}", template.display()))
                .output()
                .unwrap()
                .status
                .success());
            let common = result.repo.join(".git").canonicalize().unwrap();
            let mut store = Store::open(&result.capture).unwrap();
            store
                .approve(Binding {
                    repository_key: capture::hash(common.to_string_lossy().as_bytes()),
                    remote_hash: capture::hash(""),
                    hint_hash: capture::hash("no-mapping-hint"),
                    id: String::new(),
                    version: 0,
                    workspace_id: "co_synthetic".into(),
                    owner_id: "synthetic_owner".into(),
                    origin_hash: capture::hash("synthetic_origin"),
                    project_id: "synthetic_project".into(),
                    task_id: "synthetic_task".into(),
                    agents: vec!["codex".into()],
                    paused: false,
                    revoked: false,
                })
                .unwrap();
            result
        }
        fn command(&self, program: &Path) -> Command {
            let checked = super::isolation::checked(
                self.root.parent().unwrap(),
                &self.root,
                Some(&self.home),
                Some(&self.capture),
                Some(&self.core),
            )
            .expect("non-isolated artifact command refused");
            assert_eq!(checked.core, self.core);
            let mut cmd = Command::new(program);
            cmd.env_clear()
                .current_dir(&self.repo)
                .env("HOME", &checked.home)
                .env("CODEX_HOME", checked.home.join(".codex"))
                .env("CLAUDE_CONFIG_DIR", checked.home.join(".claude"))
                .env("KEITO_CAPTURE_HOME", &checked.capture)
                .env("XDG_CONFIG_HOME", checked.home.join("config"))
                .env("APPDATA", checked.home.join("AppData"))
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("PATH", &self.bin)
                .env("KEITO_CAPTURE_CORE_SHA256", &self.core_hash);
            cmd
        }
        fn input(&self, session: &str) -> Vec<u8> {
            serde_json::to_vec(&json!({"cwd":self.repo,"session_id":session,"turn_id":format!("turn_{session}"),"prompt":"PRIVATE_QUALIFICATION_CANARY"})).unwrap()
        }
        fn flags(&self) -> usize {
            fs::read_dir(self.capture.join("capture-invocations"))
                .map(|v| v.count())
                .unwrap_or(0)
        }
        fn observe_guard(&self, event: &str, input: &[u8]) -> Observation {
            let mut cmd = self.command(Path::new("/bin/bash"));
            cmd.env("KEITO_CLI_BIN", &self.core)
                .env("KEITO_CAPTURE_GUARD_BIN", &self.guard)
                .arg(self.root.join("dispatch.sh"))
                .args([
                    "--agent",
                    "codex",
                    "--event",
                    event,
                    "--harness-version",
                    "0.167.0",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let start = Instant::now();
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(error) => {
                    return Observation {
                        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                        success: false,
                        exit_code: None,
                        stdout_bytes: 0,
                        stderr_bytes: 0,
                        spawn_error: error.raw_os_error(),
                        flags: self.flags(),
                    }
                }
            };
            let _ = child.stdin.take().unwrap().write_all(input);
            let output = child.wait_with_output().unwrap();
            let elapsed = start.elapsed();
            Observation {
                elapsed_ms: elapsed.as_secs_f64() * 1000.0,
                success: output.status.success(),
                exit_code: output.status.code(),
                stdout_bytes: output.stdout.len(),
                stderr_bytes: output.stderr.len(),
                spawn_error: None,
                flags: self.flags(),
            }
        }
        fn assert_guard(observation: &Observation) {
            assert!(
                observation.success
                    && observation.stdout_bytes == 0
                    && observation.stderr_bytes == 0
                    && observation.spawn_error.is_none(),
                "artifact dispatch execution failed"
            );
            assert!(
                observation.elapsed_ms < 1000.0,
                "artifact host deadline: {}ms",
                observation.elapsed_ms
            );
        }
        fn guard(&self, event: &str, input: &[u8]) -> Duration {
            let observation = self.observe_guard(event, input);
            Self::assert_guard(&observation);
            Duration::from_secs_f64(observation.elapsed_ms / 1000.0)
        }
        fn assert_private_state(&self) {
            fn visit(path: &Path) {
                for entry in fs::read_dir(path).unwrap() {
                    let path = entry.unwrap().path();
                    let metadata = fs::symlink_metadata(&path).unwrap();
                    assert!(!metadata.file_type().is_symlink());
                    if metadata.is_dir() {
                        visit(&path);
                    } else {
                        let bytes = fs::read(path).unwrap();
                        assert!(
                            !bytes
                                .windows(b"PRIVATE_QUALIFICATION_CANARY".len())
                                .any(|v| v == b"PRIVATE_QUALIFICATION_CANARY"),
                            "raw hook canary persisted in capture state"
                        );
                    }
                }
            }
            visit(&self.capture);
        }
        fn direct_ack(&self, input: &[u8], nonce: &str) -> Vec<u8> {
            self.direct_ack_replacement(input, nonce, false)
        }
        fn direct_ack_replacement(
            &self,
            input: &[u8],
            nonce: &str,
            replace_after_image_open: bool,
        ) -> Vec<u8> {
            let flags = self.capture.join("capture-invocations");
            fs::create_dir_all(&flags).unwrap();
            fs::set_permissions(&flags, fs::Permissions::from_mode(0o700)).unwrap();
            let path = flags.join(format!("{nonce}.pending"));
            let mut marker = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap();
            marker
                .set_permissions(fs::Permissions::from_mode(0o600))
                .unwrap();
            marker.write_all(PENDING).unwrap();
            marker.sync_all().unwrap();
            assert_eq!(
                unsafe { libc::flock(marker.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            let image = fs::File::open(&self.core).unwrap();
            let image_fd = image.as_raw_fd();
            if replace_after_image_open {
                let replacement = self.root.join("replacement-core");
                fs::copy(&self.core, &replacement).unwrap();
                fs::rename(replacement, &self.core).unwrap();
            }
            let mut pipe = [0; 2];
            assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
            let writer = pipe[1];
            let reader = pipe[0];
            let inventory = unsafe { libc::opendir(c"/dev/fd".as_ptr()) };
            assert!(!inventory.is_null());
            let own_inventory = unsafe { libc::dirfd(inventory) };
            let mut inherited = Vec::new();
            loop {
                let entry = unsafe { libc::readdir(inventory) };
                if entry.is_null() {
                    break;
                }
                let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
                if let Some(fd) = name.to_str().ok().and_then(|v| v.parse::<i32>().ok()) {
                    if fd >= 4 && fd != own_inventory && fd != image_fd {
                        inherited.push(fd);
                    }
                }
            }
            unsafe {
                libc::closedir(inventory);
            }
            let mut cmd = self.command(&self.core);
            cmd.env("KEITO_CAPTURE_INVOCATION_NONCE", nonce)
                .env("KEITO_CAPTURE_ACK_FD", "3")
                .env("KEITO_CAPTURE_IMAGE_FD", image_fd.to_string())
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
                .stderr(Stdio::piped());
            unsafe {
                cmd.pre_exec(move || {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::dup2(writer, 3) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::fcntl(3, libc::F_SETFD, 0) < 0
                        || libc::fcntl(image_fd, libc::F_SETFD, 0) < 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                    for fd in &inherited {
                        libc::close(*fd);
                    }
                    Ok(())
                });
            }
            let mut child = cmd.spawn().unwrap();
            unsafe {
                libc::close(writer);
            }
            let _ = child.stdin.take().unwrap().write_all(input);
            let output = child.wait_with_output().unwrap();
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains("PRIVATE_QUALIFICATION_CANARY")
            );
            let mut ack = Vec::new();
            unsafe { fs::File::from_raw_fd(reader) }
                .read_to_end(&mut ack)
                .unwrap();
            if !ack.is_empty() {
                assert!(output.status.success());
            }
            drop(marker);
            ack
        }
    }
    pub fn run() {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        assert_eq!(
            args.len(),
            2,
            "qualify-capture <fresh extracted artifact directory> <canonical private run root>"
        );
        let source = PathBuf::from(&args[0]).canonicalize().unwrap();
        let owner = PathBuf::from(&args[1]).canonicalize().unwrap();
        let meta = fs::symlink_metadata(&owner).unwrap();
        assert!(
            meta.is_dir()
                && meta.uid() == unsafe { libc::geteuid() }
                && meta.mode() & 0o777 == 0o700
                && source.starts_with(&owner),
            "source/run root must be isolated and run-owned"
        );
        let mut random = [0u8; 16];
        assert_eq!(
            unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) },
            0
        );
        let nonce: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let run = owner.join(format!("paired-qualification-{nonce}"));
        fs::create_dir(&run).unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
        eprintln!("Preserved qualification evidence: {}", run.display());
        // FIRST execution of the extracted/copied pair. No version/help/runtime
        // warm-up is permitted before this qualified actual hook invocation.
        let positive = Case::new(&source, &run, "first");
        let first_observation = positive.observe_guard("prompt", &positive.input("cold"));
        fs::write(
            run.join("first-probe.json"),
            serde_json::to_vec_pretty(&first_observation).unwrap(),
        )
        .unwrap();
        Case::assert_guard(&first_observation);
        let first = Duration::from_secs_f64(first_observation.elapsed_ms / 1000.0);
        assert_eq!(
            positive.flags(),
            0,
            "first real core failed exact durable ACK/exit0"
        );
        assert_eq!(
            Store::open(&positive.capture)
                .unwrap()
                .status()
                .unwrap()
                .unfinished,
            1
        );
        positive.assert_private_state();
        positive.guard("stop", &positive.input("cold"));
        assert_eq!(positive.flags(), 0);
        let ack = Case::new(&source, &run, "exact-ack");
        assert_eq!(
            ack.direct_ack(&ack.input("ack"), &capture::hash("qualified_ack")),
            b"KEITO_CAPTURE_DURABLE_V1\n"
        );
        ack.assert_private_state();
        let malformed = Case::new(&source, &run, "malformed");
        assert!(malformed
            .direct_ack(
                b"{PRIVATE_QUALIFICATION_CANARY",
                &capture::hash("malformed")
            )
            .is_empty());
        assert_eq!(malformed.flags(), 1);
        assert_eq!(
            Store::open(&malformed.capture)
                .unwrap()
                .status()
                .unwrap()
                .unfinished,
            0
        );
        malformed.assert_private_state();
        for changed in ["core", "guard", "manifest"] {
            let case = Case::new(&source, &run, &format!("changed-{changed}"));
            match changed {
                "core" => fs::write(&case.core, b"replaced inert core fixture").unwrap(),
                "guard" => fs::write(&case.guard, b"replaced inert guard fixture").unwrap(),
                _ => fs::write(
                    case.core
                        .parent()
                        .unwrap()
                        .join("capture-runtime-manifest.json"),
                    b"{}",
                )
                .unwrap(),
            }
            if changed == "core" {
                case.guard("prompt", &case.input("changed"));
            } else {
                assert!(case
                    .direct_ack(&case.input("changed"), &capture::hash(changed))
                    .is_empty());
            }
            case.assert_private_state();
            assert_eq!(case.flags(), 1);
            assert_eq!(
                Store::open(&case.capture)
                    .unwrap()
                    .status()
                    .unwrap()
                    .unfinished,
                0,
                "changed pair accepted a boundary"
            );
        }
        let replaced = Case::new(&source, &run, "replaced-after-image-open");
        assert!(replaced
            .direct_ack_replacement(
                &replaced.input("replaced"),
                &capture::hash("replaced_image"),
                true
            )
            .is_empty());
        assert_eq!(replaced.flags(), 1);
        assert_eq!(
            Store::open(&replaced.capture)
                .unwrap()
                .status()
                .unwrap()
                .unfinished,
            0
        );
        replaced.assert_private_state();
        let mut milliseconds = Vec::new();
        for n in 0..100 {
            let session = format!("latency_{n}");
            milliseconds.push(
                positive
                    .guard("prompt", &positive.input(&session))
                    .as_secs_f64()
                    * 1000.0,
            );
            positive.guard("stop", &positive.input(&session));
            assert_eq!(positive.flags(), 0);
        }
        positive.assert_private_state();
        milliseconds.sort_by(f64::total_cmp);
        let p95 = milliseconds[94];
        let result = json!({"first_ms":first.as_secs_f64()*1000.0,"p95_ms":p95,"samples":100,"first_host_deadline_passed":first<Duration::from_secs(1),"p95_target_passed":p95<100.0,"actual_core_exact_ack":true,"raw_canary_absent_from_capture_state":true,"network_deny":std::env::var("KEITO_QUALIFICATION_NETWORK_DENY").unwrap_or_else(|_|"unavailable".into()),"malformed_no_ack":true,"changed_core_guard_manifest_refused":true,"replaced_image_before_entry_refused":true,"evidence_directory":run});
        fs::write(
            run.join("qualification.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        assert!(
            p95 < 100.0,
            "actual paired artifact p95 exceeds100ms; preserve first failure"
        );
    }
}
fn main() {
    #[cfg(unix)]
    qualification::run();
    #[cfg(not(unix))]
    {
        eprintln!("Native capture artifact qualification is unsupported on this platform");
        std::process::exit(1);
    }
}
