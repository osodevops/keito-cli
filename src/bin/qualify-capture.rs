//! Development-only paired-artifact acceptance. No harness, credentials,
//! network service, installed profile or real CLI fallback is used.
#[cfg(unix)]
#[path = "../../tests/support/mutation_environment.rs"]
mod isolation;
#[cfg(unix)]
mod qualification {
    use keito_cli::{
        capture::{self, Binding, Store},
        process_ownership::{io::IoPlan, scope::Scope},
    };
    use serde_json::{json, Value};
    use std::{
        fs,
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::{
                fs::{MetadataExt, PermissionsExt},
                process::{CommandExt, ExitStatusExt},
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
    const ACK: &[u8] = b"KEITO_CAPTURE_DURABLE_V1\n";
    const STDERR_LIMIT: usize = 64 * 1024;
    fn direct_clock() -> std::io::Result<u64> {
        let mut clock = std::mem::MaybeUninit::<libc::timespec>::uninit();
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, clock.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let clock = unsafe { clock.assume_init() };
        u64::try_from(clock.tv_sec)
            .ok()
            .and_then(|s| s.checked_mul(1_000_000_000))
            .and_then(|s| s.checked_add(clock.tv_nsec as u64))
            .ok_or_else(|| std::io::Error::other("qualifier clock overflow"))
    }
    fn direct_sync_marker(marker: &fs::File) -> std::io::Result<()> {
        // Match native_capture_guard::sync: POSIX fsync, not Apple's
        // F_FULLFSYNC used by Rust File::sync_all. Never refresh leaf/parent ends.
        let started = Instant::now();
        loop {
            if unsafe { libc::fsync(marker.as_raw_fd()) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted
                || started.elapsed() >= Duration::from_millis(25)
            {
                return Err(error);
            }
        }
    }
    fn direct_admission_fresh(at: u64, leaf_end: u64) -> bool {
        // The public cooperative API has the parent ceiling; preserve the
        // guard's earlier leaf admission reserve at both caller and exec edge.
        at < leaf_end.saturating_sub(50_000_000)
    }
    fn direct_close_inventory(image_fd: i32, inventory_fd: i32, fds: &[i32]) -> Vec<i32> {
        assert!((5..=1024).contains(&image_fd));
        fds.iter()
            .copied()
            .filter(|fd| *fd >= 4 && *fd != inventory_fd && *fd != image_fd)
            .collect()
    }
    fn direct_pipe() -> (fs::File, OwnedFd) {
        let mut pipe = [-1; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let reader = unsafe { fs::File::from_raw_fd(pipe[0]) };
        let writer = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
        for fd in [reader.as_raw_fd(), writer.as_raw_fd()] {
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        assert_eq!(
            unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
            0
        );
        (reader, writer)
    }
    fn direct_read(
        reader: &mut fs::File,
        bytes: &mut Vec<u8>,
        eof: &mut bool,
        limit: usize,
    ) -> std::io::Result<()> {
        if *eof {
            return Ok(());
        }
        // One bounded read per owner-pump turn. One extra byte detects excess.
        let mut part = [0u8; 256];
        let remaining = (limit + 1).saturating_sub(bytes.len()).min(part.len());
        if remaining == 0 {
            return Err(std::io::Error::other("qualifier output limit"));
        }
        match reader.read(&mut part[..remaining]) {
            Ok(0) => *eof = true,
            Ok(n) => bytes.extend_from_slice(&part[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
        if bytes.len() > limit {
            return Err(std::io::Error::other("qualifier output limit"));
        }
        Ok(())
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
        fn use_guarded_pair(&mut self, guarded: &Self) {
            // The unchanged first probe already exercised this exact image pair
            // through the cold guard entry. Direct-ACK is a protocol fixture,
            // not a second cold keeper-start test on a never-dispatched guard.
            // Keep this case's fresh HOME/capture/repository and pending marker.
            assert_eq!(self.root.parent(), guarded.root.parent());
            assert_eq!(self.core_hash, guarded.core_hash);
            self.core = guarded.core.clone();
            self.guard = guarded.guard.clone();
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
            let origin = direct_clock().unwrap();
            let leaf_end = origin.checked_add(500_000_000).unwrap();
            let parent_end = origin.checked_add(600_000_000).unwrap();
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
            direct_sync_marker(&marker).unwrap();
            assert_eq!(
                unsafe { libc::flock(marker.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            let original_image = fs::File::open(&self.core).unwrap();
            // Owner attachment is fixed at4. Bootstrap requires image5..1024.
            let image_fd =
                unsafe { libc::fcntl(original_image.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 5) };
            assert!((5..=1024).contains(&image_fd));
            let image = unsafe { fs::File::from_raw_fd(image_fd) };
            drop(original_image);
            if replace_after_image_open {
                let replacement = self.root.join("replacement-core");
                fs::copy(&self.core, &replacement).unwrap();
                fs::rename(replacement, &self.core).unwrap();
            }
            let (mut reader, writer) = direct_pipe();
            // Scope's fault path intentionally withholds take_result output.
            // Retain stderr independently for the existing privacy assertion;
            // stdin/stdout and the internal stderr pipe remain Scope-owned.
            let (mut error_reader, error_writer) = direct_pipe();
            let write_fd = writer.as_raw_fd();
            let error_fd = error_writer.as_raw_fd();
            let inventory = unsafe { libc::opendir(c"/dev/fd".as_ptr()) };
            assert!(!inventory.is_null());
            let own_inventory = unsafe { libc::dirfd(inventory) };
            let mut descriptors = Vec::new();
            loop {
                let entry = unsafe { libc::readdir(inventory) };
                if entry.is_null() {
                    break;
                }
                let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
                if let Some(fd) = name.to_str().ok().and_then(|v| v.parse::<i32>().ok()) {
                    descriptors.push(fd);
                }
            }
            let inherited = direct_close_inventory(image_fd, own_inventory, &descriptors);
            unsafe {
                libc::closedir(inventory);
            }
            let mut cmd = self.command(&self.core);
            cmd.env("KEITO_CAPTURE_INVOCATION_NONCE", nonce)
                .env("KEITO_CAPTURE_ACK_FD", "3")
                .env("KEITO_CAPTURE_IMAGE_FD", image_fd.to_string())
                .env("KEITO_CAPTURE_LEAF_END_NS", leaf_end.to_string())
                .args([
                    "capture",
                    "hook",
                    "--agent",
                    "codex",
                    "--event",
                    "prompt",
                    "--harness-version",
                    "0.167.0",
                ]);
            unsafe {
                cmd.pre_exec(move || {
                    if !direct_admission_fresh(direct_clock()?, leaf_end) {
                        return Err(std::io::Error::from_raw_os_error(libc::ETIMEDOUT));
                    }
                    if libc::setsid() < 0
                        || libc::dup2(write_fd, 3) < 0
                        || libc::dup2(error_fd, 2) < 0
                    {
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
                    // Scope appends its owner4 callback AFTER this callback.
                    // Its freshly allocated attachment descriptors were not in
                    // our earlier inventory; no private owner tuple is forged.
                    Ok(())
                });
            }
            let mut scope = Scope::local_entry(parent_end).unwrap();
            assert!(direct_admission_fresh(direct_clock().unwrap(), leaf_end));
            let child = scope
                .start_cooperative_with_io(cmd, IoPlan::new(input.to_vec(), 4096, 4096).unwrap())
                .unwrap();
            drop(writer);
            drop(error_writer);
            let mut ack = Vec::with_capacity(ACK.len() + 1);
            let mut stderr = Vec::new();
            let (mut eof, mut error_eof) = (false, false);
            let mut read_fault = false;
            let mut observed_before_parent = false;
            loop {
                scope.poll();
                read_fault |= direct_read(&mut reader, &mut ack, &mut eof, ACK.len()).is_err();
                read_fault |=
                    direct_read(&mut error_reader, &mut stderr, &mut error_eof, STDERR_LIMIT)
                        .is_err();
                let report = scope.report();
                let slot = report.slots.first().unwrap();
                // Negative bootstrap/FAILED peers need not provide a clean
                // terminal. Independently retain original wait and pipe facts.
                let fresh = direct_clock().unwrap();
                if slot.waits_known
                    && slot.io.complete()
                    && eof
                    && error_eof
                    && (scope.slot_complete(&child) || scope.failed())
                    && fresh < parent_end
                {
                    observed_before_parent = true;
                    break;
                }
                if fresh >= parent_end {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let output = if scope.slot_complete(&child) {
                Some(scope.take_result(&child).unwrap())
            } else {
                None
            };
            let report = scope.finish();
            let slot = report.slots.first().unwrap();
            fs::write(
                self.root.join("direct-ack-probe.json"),
                serde_json::to_vec_pretty(&json!({
                    "ack_bytes":ack.len(), "ack_eof":eof, "stderr_eof":error_eof,
                    "observed_before_parent":observed_before_parent,
                    "stderr_bytes":stderr.len(), "read_fault":read_fault,
                    "waits_known":slot.waits_known, "exit_code":slot.status.and_then(|s|s.code()),
                    "signal":slot.status.and_then(|s|s.signal()), "terminal_eof":slot.terminal_eof,
                    "stdin_closed":slot.io.stdin_closed, "stdout_eof":slot.io.stdout_eof,
                    "scope_stderr_eof":slot.io.stderr_eof, "io_fault":slot.io.first_fault,
                    "slot_fault":slot.first_fault, "scope_fault":report.first_fault.map(|e|e.0),
                    "whole_clean":report.clean, "all_resources":report.all_resources,
                    "leaf_end_ns":leaf_end, "parent_end_ns":parent_end,
                    "finality_limit":"negative refusal proves only independently observed facts; no clean-terminal or descendant proof is synthesized"
                })).unwrap(),
            ).unwrap();
            assert!(
                !read_fault && eof && error_eof && observed_before_parent,
                "direct ACK/stderr stream not boundedly settled"
            );
            assert!(
                slot.waits_known && slot.status.is_some() && slot.io.complete(),
                "original child wait/stdio finality unknown"
            );
            assert!(!String::from_utf8_lossy(&stderr).contains("PRIVATE_QUALIFICATION_CANARY"));
            if !ack.is_empty() {
                assert!(
                    slot.io.first_fault.is_none(),
                    "ACK with direct core stdio fault"
                );
                assert!(
                    report.clean && slot.terminal_eof,
                    "ACK without cooperative settlement"
                );
                let output = output
                    .as_ref()
                    .expect("ACK without retained command result");
                assert!(output.status.success());
                assert!(output.output.stdout.is_empty() && output.output.stderr.is_empty());
            }
            drop(image);
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
        let mut ack = Case::new(&source, &run, "exact-ack");
        ack.use_guarded_pair(&positive);
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
    #[cfg(test)]
    mod direct_fixture_tests {
        use super::*;
        #[test]
        fn image_never_uses_ack3_or_owner4_and_inventory_precedes_owner_registration() {
            assert_eq!(
                direct_close_inventory(5, 9, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
                vec![4, 6, 7, 8]
            );
            assert_eq!(
                direct_close_inventory(8, 9, &[3, 4, 5, 6, 7, 8, 9]),
                vec![4, 5, 6, 7]
            );
            assert!(std::panic::catch_unwind(|| direct_close_inventory(4, 9, &[])).is_err());
        }
        #[test]
        fn normal_hook_source_keeps_missing_owner_and_image_fd_refusals() {
            let hook = include_str!("../commands/capture.rs");
            assert!(hook.contains("command_owner.as_mut().ok_or_else"));
            assert!(hook.contains("Capture hook requires a verified guarded command owner"));
            let bootstrap = include_str!("../capture_runtime_pair.rs");
            assert!(bootstrap.contains("(5..=1024).contains(v)"));
            let owner = include_str!("../capture_command_owner.rs");
            assert!(owner.contains("std::env::var(LEAF_END_ENV).map_err"));
        }
        fn isolated_case(cohort: &str, name: &str, hash: &str) -> Case {
            let root = PathBuf::from(cohort).join(name);
            Case {
                home: root.join("home"),
                capture: root.join("home/capture"),
                core: root.join("artifact/keito"),
                guard: root.join("artifact/keito-capture-guard"),
                bin: root.join("bin"),
                repo: root.join("repo"),
                core_hash: hash.into(),
                root,
            }
        }
        #[test]
        fn direct_ack_reuses_only_guarded_pair_and_preserves_fresh_case_state() {
            let guarded = isolated_case("/owned/cohort", "first", "same_core");
            let mut ack = isolated_case("/owned/cohort", "exact-ack", "same_core");
            let state = (
                ack.root.clone(),
                ack.home.clone(),
                ack.capture.clone(),
                ack.repo.clone(),
                ack.bin.clone(),
            );
            ack.use_guarded_pair(&guarded);
            assert_eq!(ack.core, guarded.core);
            assert_eq!(ack.guard, guarded.guard);
            assert_eq!((ack.root, ack.home, ack.capture, ack.repo, ack.bin), state);
        }
        #[test]
        fn direct_ack_rejects_pair_from_another_cohort_or_core_identity() {
            let guarded = isolated_case("/owned/cohort", "first", "same_core");
            let mut wrong_hash = isolated_case("/owned/cohort", "exact-ack", "different_core");
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || wrong_hash.use_guarded_pair(&guarded)
            ))
            .is_err());
            let mut foreign = isolated_case("/foreign/cohort", "exact-ack", "same_core");
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || foreign.use_guarded_pair(&guarded)
            ))
            .is_err());
        }
        #[test]
        fn leaf_admission_reserve_uses_the_original_absolute_bound() {
            assert!(direct_admission_fresh(449_999_999, 500_000_000));
            assert!(!direct_admission_fresh(450_000_000, 500_000_000));
            assert!(!direct_admission_fresh(500_000_000, 500_000_000));
            assert!(!direct_admission_fresh(0, 1));
        }
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
