//! Developer-only finite PUBLIC GroupRegistry API actor. Never installed.
//! Authored source, not runtime acceptance. Only the reserved keeper has group
//! authority. Lifeline EOF is QA metadata, never product output EOF.
#[cfg(unix)]
mod unix {
    use keito_cli::{
        source_qa_protocol::SharedOwner,
        source_qa_scope::{GroupRegistry, RegistryReport, SlotId},
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::{
        fs::{self, File, OpenOptions},
        io::{self, Read, Write},
        os::{
            fd::AsRawFd,
            unix::{
                fs::{MetadataExt, OpenOptionsExt},
                net::UnixStream,
                process::{CommandExt, ExitStatusExt},
            },
        },
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::Duration,
    };
    type R<T> = Result<T, &'static str>;
    const NS: u64 = 1_000_000_000;
    const CASES: [&str; 4] = [
        "api-clean-two",
        "api-publication-fault",
        "api-edge-fault",
        "api-expired-bound",
    ];
    fn clock() -> R<u64> {
        let mut t = std::mem::MaybeUninit::<libc::timespec>::uninit();
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, t.as_mut_ptr()) } != 0 {
            return Err("CLOCK");
        }
        let t = unsafe { t.assume_init() };
        (t.tv_sec as u64)
            .checked_mul(NS)
            .and_then(|n| n.checked_add(t.tv_nsec as u64))
            .ok_or("CLOCK")
    }
    fn nap() {
        std::thread::sleep(Duration::from_millis(1));
    }
    fn hex(s: &str, n: usize) -> bool {
        s.len() == n
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    fn owned(p: &Path, root: &Path, dir: bool) -> R<()> {
        if !p.is_absolute() || !p.starts_with(root) || p.canonicalize().ok().as_deref() != Some(p) {
            return Err("PATH");
        }
        let m = fs::symlink_metadata(p).map_err(|_| "PATH")?;
        if m.file_type().is_symlink()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o777 != 0o700
            || (dir && !m.is_dir())
            || (!dir && (!m.is_file() || m.nlink() != 1))
        {
            return Err("MODE");
        }
        Ok(())
    }
    fn digest(p: &Path) -> R<String> {
        let mut f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(p)
            .map_err(|_| "HASH_OPEN")?;
        let before = f.metadata().map_err(|_| "HASH_META")?;
        if !before.is_file() || before.nlink() != 1 || before.len() > 128 * 1024 * 1024 {
            return Err("HASH_META");
        }
        let mut h = Sha256::new();
        let mut b = [0u8; 65536];
        let mut total = 0;
        loop {
            let n = f.read(&mut b).map_err(|_| "HASH_READ")?;
            if n == 0 {
                break;
            }
            total += n;
            if total > 128 * 1024 * 1024 {
                return Err("HASH_LIMIT");
            }
            h.update(&b[..n]);
        }
        let after = f.metadata().map_err(|_| "HASH_META")?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return Err("HASH_DRIFT");
        }
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }
    fn exclusive(path: &Path, value: &Value) -> R<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| "RESULT_EXCLUSIVE")?;
        let b = serde_json::to_vec_pretty(&value).map_err(|_| "RESULT_JSON")?;
        f.write_all(&b)
            .and_then(|_| f.write_all(b"\n"))
            .and_then(|_| f.flush())
            .map_err(|_| "RESULT_WRITE")?;
        if unsafe { libc::fsync(f.as_raw_fd()) } != 0 {
            return Err("RESULT_FSYNC");
        }
        Ok(())
    }
    // Retain the caller's immutable cutpoint by reference. Durable means the
    // exclusive writer completed write+flush+fsync, never merely create succeeded.
    struct Publication {
        durable: bool,
        fallback_emitted: bool,
        errors: Vec<&'static str>,
    }
    fn publish_snapshot<W, F>(
        snapshot: &Value,
        phase: &'static str,
        mut write: W,
        mut fallback: F,
    ) -> Publication
    where
        W: FnMut(&Value) -> R<()>,
        F: FnMut(&Value) -> R<()>,
    {
        let mut receipt = Publication {
            durable: false,
            fallback_emitted: false,
            errors: vec![],
        };
        match write(snapshot) {
            Ok(()) => receipt.durable = true,
            Err(reason) => {
                receipt.errors.push(reason);
                let packet = json!({"publication":phase,"publication_fault":reason,"case_pass":false,"snapshot":snapshot});
                match fallback(&packet) {
                    Ok(()) => receipt.fallback_emitted = true,
                    Err(reason) => receipt.errors.push(reason),
                }
            }
        }
        receipt
    }
    fn emit_fact_fallback(value: &Value) -> R<()> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| "FACT_JSON")?;
        if bytes.len() > 16384 {
            return Err("FACT_LIMIT");
        }
        bytes.push(b'\n');
        // The reviewed outer invocation binds stdout/stderr to private regular
        // files. A failed sink never skips natural containment/wait collection.
        let out = std::io::stdout();
        let result = {
            let mut sink = out.lock();
            sink.write_all(&bytes).and_then(|()| sink.flush())
        };
        if result.is_ok() {
            return Ok(());
        }
        let err = std::io::stderr();
        let mut sink = err.lock();
        sink.write_all(&bytes)
            .and_then(|()| sink.flush())
            .map_err(|_| "FACT_OUTPUT")
    }
    fn private_role(root: &Path) -> R<()> {
        if std::env::var("KEITO_ACTOR_POLICY_CONFIRMED").as_deref()
            != Ok("source-registry-api-only")
        {
            return Err("POLICY_PREREQUISITE");
        }
        owned(root, root, true)?;
        for n in [
            "home",
            "home/.codex",
            "home/.claude",
            "home/capture",
            "home/config",
            "home/state",
            "home/AppData",
            "tmp",
            "bin",
        ] {
            owned(&root.join(n), root, true)?;
        }
        // This refusal flag supplements, but does not prove, the inherited policy.
        for (key, relative) in [
            ("HOME", "home"),
            ("CODEX_HOME", "home/.codex"),
            ("CLAUDE_CONFIG_DIR", "home/.claude"),
            ("KEITO_CAPTURE_HOME", "home/capture"),
            ("XDG_CONFIG_HOME", "home/config"),
            ("XDG_STATE_HOME", "home/state"),
            ("APPDATA", "home/AppData"),
            ("TMPDIR", "tmp"),
            ("PATH", "bin"),
        ] {
            if std::env::var_os(key).map(PathBuf::from).as_ref() != Some(&root.join(relative)) {
                return Err("PRIVATE_ENV");
            }
        }
        if std::env::vars_os().count() != 10 {
            return Err("ENV_KEYS");
        }
        Ok(())
    }
    fn unix_nonblocking_fd(fd: i32) -> R<()> {
        let mut kind: libc::c_int = 0;
        let mut n = std::mem::size_of_val(&kind) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut libc::c_int).cast(),
                &mut n,
            )
        } != 0
            || kind != libc::SOCK_STREAM
        {
            return Err("ROLE_FD");
        }
        let mut address = std::mem::MaybeUninit::<libc::sockaddr_storage>::zeroed();
        let mut length = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if unsafe { libc::getsockname(fd, address.as_mut_ptr().cast(), &mut length) } != 0
            || unsafe { address.assume_init() }.ss_family as i32 != libc::AF_UNIX
        {
            return Err("ROLE_FD");
        }
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || flags & libc::O_NONBLOCK == 0 {
            return Err("ROLE_FD_BLOCKING");
        }
        Ok(())
    }

    // The post-fork member uses only libc, no allocation/runtime/destructors.
    fn finite_leader(args: &[String]) -> R<()> {
        if args.len() != 5 || !matches!(args[1].as_str(), "0" | "1") {
            return Err("LEADER_ARGS");
        }
        let root = PathBuf::from(&args[0]);
        private_role(&root)?;
        if std::env::current_exe().map_err(|_| "SELF")? != root.join("bin/actor") {
            return Err("SELF");
        }
        let ready = args[2].parse::<i32>().map_err(|_| "LEADER_ARGS")?;
        let live = args[3].parse::<i32>().map_err(|_| "LEADER_ARGS")?;
        let safety = args[4].parse::<u64>().map_err(|_| "LEADER_ARGS")?;
        let at = clock()?;
        if !(4..=1024).contains(&ready)
            || !(4..=1024).contains(&live)
            || ready == live
            || safety <= at
            || safety - at > 5 * NS
        {
            return Err("LEADER_ARGS");
        }
        unix_nonblocking_fd(ready)?;
        unix_nonblocking_fd(live)?;
        let child = unsafe { libc::fork() };
        if child < 0 {
            return Err("FORK");
        }
        if child == 0 {
            unsafe {
                if libc::signal(libc::SIGTERM, libc::SIG_IGN) == libc::SIG_ERR {
                    libc::_exit(91);
                }
                if libc::write(ready, b"R".as_ptr().cast(), 1) != 1 {
                    libc::_exit(92);
                }
                libc::close(ready);
                let delay = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 1_000_000,
                };
                for _ in 0..6000 {
                    let mut t = std::mem::MaybeUninit::<libc::timespec>::uninit();
                    if libc::clock_gettime(libc::CLOCK_MONOTONIC, t.as_mut_ptr()) != 0 {
                        let _ = libc::write(live, b"F".as_ptr().cast(), 1);
                        libc::_exit(93);
                    }
                    let t = t.assume_init();
                    if (t.tv_sec as u64) * NS + t.tv_nsec as u64 >= safety {
                        break;
                    }
                    libc::nanosleep(&delay, std::ptr::null_mut());
                }
                let _ = libc::write(live, b"N".as_ptr().cast(), 1);
                libc::close(live);
                libc::_exit(0);
            }
        }
        unsafe {
            libc::close(ready);
            libc::close(live);
        }
        Ok(()) // Member is an orphan, never reported as reaped by this controller.
    }

    fn registry_facts(report: &RegistryReport) -> Value {
        json!({"first_fault":report.first_fault.map(|e|e.0),
        "secondary_faults":report.secondary_faults.iter().map(|e|e.0).collect::<Vec<_>>(),
        "admission_closed":report.admission_closed,"retained_slots":report.retained_slots,
        "cleanup_end":report.cleanup_end,"all_group_proofs":report.all_group_proofs,
        "all_direct_waits_known":report.all_direct_waits_known,
        "finish_called":report.finish_called,"output_eof_proven":report.output_eof_proven,
        "groups":report.groups.iter().map(|g|json!({"slot":g.slot,"proof":g.group_proof,
            "keeper_pid":g.facts.keeper_pid,"workload_pid":g.facts.workload_pid,
            "first_fault":g.facts.first_fault,"secondary_faults":g.facts.secondary_faults,
            "workload_spawned":g.facts.workload_spawned,
            "keeper_wait":g.facts.keeper_status.map(|s|json!({"code":s.code(),"signal":s.signal(),"success":s.success()})),
            "workload_wait":g.facts.workload_status.map(|s|json!({"code":s.code(),"signal":s.signal(),"success":s.success()})),
            "group_absent":g.facts.group_absent,"release_sent":g.facts.release_sent,
            "terminal_eof":g.facts.terminal_eof})).collect::<Vec<_>>()})
    }

    struct Observation {
        ready: UnixStream,
        live: UnixStream,
        ready_at: Option<u64>,
        eof_at: Option<u64>,
        natural: bool,
        marker_checked: bool,
    }
    impl Observation {
        fn poll(&mut self) -> R<()> {
            if self.ready_at.is_none() {
                let mut b = [0u8; 2];
                match self.ready.read(&mut b) {
                    Ok(1) if b[0] == b'R' => self.ready_at = Some(clock()?),
                    Ok(0) => return Err("READY_EOF"),
                    Ok(_) => return Err("READY_SCHEMA"),
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        ()
                    }
                    Err(_) => return Err("READY_READ"),
                }
            }
            if self.eof_at.is_none() {
                let mut b = [0u8; 2];
                match self.live.read(&mut b) {
                    Ok(0) => self.eof_at = Some(clock()?),
                    Ok(1) if b[0] == b'N' => self.natural = true,
                    Ok(1) if b[0] == b'F' => return Err("MEMBER_CLOCK"),
                    Ok(_) => return Err("LIVE_SCHEMA"),
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        ()
                    }
                    Err(_) => return Err("LIVE_READ"),
                }
            }
            Ok(())
        }
        fn facts(&self) -> Value {
            json!({"ready_at":self.ready_at,"lifeline_eof_at":self.eof_at,
            "natural_expiry_marker":self.natural,"original_preexec_preserved":self.marker_checked,
            "orphan_member_reaped":false,"product_output_eof":false})
        }
    }

    struct Run {
        registry: GroupRegistry,
        observations: Vec<Observation>,
        log: File,
        events: usize,
        hard: u64,
        safety: u64,
        first: Option<&'static str>,
        secondary: Vec<&'static str>,
    }
    impl Run {
        fn fail(&mut self, why: &'static str) {
            if self.first.is_none() {
                self.first = Some(why);
            } else if self.first != Some(why)
                && self.secondary.len() < 8
                && !self.secondary.contains(&why)
            {
                self.secondary.push(why);
            }
        }
        fn event(&mut self, tag: &'static str, value: Value) -> R<()> {
            self.events += 1;
            if self.events > 64 {
                return Err("EVENT_LIMIT");
            }
            let b = serde_json::to_vec(&json!({"event":tag,"at":clock().ok(),"fact":value}))
                .map_err(|_| "EVENT_JSON")?;
            self.log
                .write_all(&b)
                .and_then(|_| self.log.write_all(b"\n"))
                .and_then(|_| self.log.flush())
                .and_then(|_| self.log.sync_all())
                .map_err(|_| "EVENT_WRITE")
        }
        fn poll_observations(&mut self) -> R<()> {
            let mut first = None;
            for observation in &mut self.observations {
                if let Err(e) = observation.poll() {
                    first.get_or_insert(e);
                }
            }
            first.map_or(Ok(()), Err)
        }
        fn facts(&self) -> Value {
            json!({"first_fault":self.first,"secondary_faults":self.secondary,
            "registry":registry_facts(&self.registry.report()),
            "observations":self.observations.iter().map(Observation::facts).collect::<Vec<_>>(),
            "hard_end":self.hard,"fixed_member_safety_end":self.safety,
            "controller_group_signals_or_queries":false,"raw_esrch_instrumented":false})
        }
        fn spawn(&mut self, root: &Path, keeper: &Path, index: usize) -> io::Result<SlotId> {
            self.registry.spawn_group(root, keeper, |id, report| {
                if id.index() != index || report.workload_spawned || report.keeper_pid == 0 {
                    return Err(io::Error::other("PUBLISHER_NOT_RETAINED"));
                }
                // A real exclusive write: slot1 deliberately collides only in the
                // fixed publication-fault case. Registry must retain its actual Child.
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(root.join(format!("registration-{index}.json")))?;
                let bytes =
                    serde_json::to_vec(&json!({"slot":id.index(),"keeper_pid":report.keeper_pid}))?;
                f.write_all(&bytes)?;
                f.flush()?;
                f.sync_all()
            })
        }
        fn start(&mut self, root: &Path, image: &Path, id: &SlotId, index: usize) -> R<()> {
            let (ready_r, ready_w) = UnixStream::pair().map_err(|_| "PAIR")?;
            let (live_r, live_w) = UnixStream::pair().map_err(|_| "PAIR")?;
            for s in [&ready_r, &ready_w, &live_r, &live_w] {
                s.set_nonblocking(true).map_err(|_| "NONBLOCK")?;
            }
            let marker_path = root.join(format!("preexec-{index}.txt"));
            let marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&marker_path)
                .map_err(|_| "MARKER_OPEN")?;
            let (rf, lf, mf) = (ready_w.as_raw_fd(), live_w.as_raw_fd(), marker.as_raw_fd());
            if [rf, lf, mf].iter().any(|f| !(4..=1024).contains(f)) {
                return Err("FD");
            }
            let mut command = Command::new(image);
            command
                .env_clear()
                .current_dir(root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .env("KEITO_ACTOR_POLICY_CONFIRMED", "source-registry-api-only")
                .env("HOME", root.join("home"))
                .env("CODEX_HOME", root.join("home/.codex"))
                .env("CLAUDE_CONFIG_DIR", root.join("home/.claude"))
                .env("KEITO_CAPTURE_HOME", root.join("home/capture"))
                .env("XDG_CONFIG_HOME", root.join("home/config"))
                .env("XDG_STATE_HOME", root.join("home/state"))
                .env("APPDATA", root.join("home/AppData"))
                .env("TMPDIR", root.join("tmp"))
                .env("PATH", root.join("bin"))
                .args([
                    "--finite-leader".to_owned(),
                    root.to_string_lossy().into_owned(),
                    index.to_string(),
                    rf.to_string(),
                    lf.to_string(),
                    self.safety.to_string(),
                ]);
            unsafe {
                command.pre_exec(move || {
                    if libc::write(mf, b"P".as_ptr().cast(), 1) != 1
                        || libc::fsync(mf) != 0
                        || libc::fcntl(rf, libc::F_SETFD, 0) < 0
                        || libc::fcntl(lf, libc::F_SETFD, 0) < 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            // Retain observation endpoints BEFORE consuming the original Command.
            self.observations.push(Observation {
                ready: ready_r,
                live: live_r,
                ready_at: None,
                eof_at: None,
                natural: false,
                marker_checked: false,
            });
            let result = self.registry.start(id, command);
            drop(ready_w);
            drop(live_w);
            drop(marker);
            result.map_err(|_| "START")?;
            if fs::read(marker_path).map_err(|_| "MARKER_READ")? != b"P" {
                return Err("PREEXEC_LOST");
            }
            self.observations[index].marker_checked = true;
            Ok(())
        }
        fn ready(&mut self, end: u64) -> R<()> {
            loop {
                self.poll_observations()?;
                if self
                    .observations
                    .iter()
                    .any(|v| v.eof_at.is_some() || v.natural)
                {
                    return Err("NOT_LIVE");
                }
                if clock()? >= end {
                    return Err("SETUP_BOUND");
                }
                if self.observations.iter().all(|v| v.ready_at.is_some()) {
                    return Ok(());
                }
                nap();
            }
        }
        fn fallback(&mut self) {
            // Same existing bound, no new cleanup/proof allowance. Late facts never
            // replace FIRST. Only already-owned leases may collect actual waits.
            self.registry.begin_cleanup(self.hard);
            for _ in 0..6000 {
                self.registry.poll_cleanup();
                if let Err(e) = self.poll_observations() {
                    self.fail(e);
                }
                if self.registry.report().all_direct_waits_known
                    && self.observations.iter().all(|v| v.eof_at.is_some())
                {
                    break;
                }
                if clock().is_ok_and(|at| at >= self.safety + 500_000_000) {
                    break;
                }
                nap();
            }
            // Required explicit finalization even for setup/receipt failure.
            self.registry.finish();
        }
    }

    fn binding(root: &Path, image: &Path, keeper: &Path) -> R<Value> {
        let path = root.join("binding.json");
        let m = fs::symlink_metadata(&path).map_err(|_| "BINDING")?;
        if !m.is_file()
            || m.file_type().is_symlink()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o777 != 0o600
            || m.len() > 65536
        {
            return Err("BINDING");
        }
        let b: Value = serde_json::from_slice(&fs::read(path).map_err(|_| "BINDING")?)
            .map_err(|_| "BINDING")?;
        let keys = [
            "source_commit",
            "actor_sha256",
            "keeper_sha256",
            "source_manifest_sha256",
            "actor_source_sha256",
        ];
        if b.as_object()
            .is_none_or(|o| o.len() != keys.len() || o.keys().any(|k| !keys.contains(&k.as_str())))
        {
            return Err("BINDING_SCHEMA");
        }
        if !b["source_commit"].as_str().is_some_and(|s| hex(s, 40))
            || ["source_manifest_sha256", "actor_source_sha256"]
                .iter()
                .any(|k| !b[*k].as_str().is_some_and(|s| hex(s, 64)))
            || b["actor_sha256"].as_str() != Some(digest(image)?.as_str())
            || b["keeper_sha256"].as_str() != Some(digest(keeper)?.as_str())
        {
            return Err("BINDING_MISMATCH");
        }
        Ok(b)
    }

    fn controller(args: &[String]) -> R<()> {
        if args.len() != 3 || !CASES.contains(&args[1].as_str()) {
            return Err("ARGS");
        }
        let root = PathBuf::from(&args[0]);
        private_role(&root)?;
        let image = std::env::current_exe().map_err(|_| "SELF")?;
        let keeper = root.join("bin/keeper");
        if image != root.join("bin/actor") || PathBuf::from(&args[2]) != keeper {
            return Err("IMAGE_PATH");
        }
        owned(&image, &root, false)?;
        owned(&keeper, &root, false)?;
        let bound = binding(&root, &image, &keeper)?;
        let scenario = args[1].as_str();
        if scenario == "api-publication-fault"
            && fs::read(root.join("registration-1.json")).map_err(|_| "CANARY")? != b"CANARY\n"
        {
            return Err("CANARY");
        }
        let log = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join("events.jsonl"))
            .map_err(|_| "EVENT_OPEN")?;
        if unsafe { libc::getpid() } == unsafe { libc::getsid(0) }
            || unsafe { libc::setpgid(0, 0) } != 0
        {
            return Err("CONTROLLER_GROUP");
        }
        let start = clock()?;
        let hard = start + 3 * NS;
        let safety = start + 4 * NS;
        let mut run = Run {
            registry: GroupRegistry::new(SharedOwner::default(), hard),
            observations: Vec::with_capacity(2),
            log,
            events: 0,
            hard,
            safety,
            first: None,
            secondary: Vec::new(),
        };
        let outcome = (|| -> R<Value> {
            let a = run.spawn(&root, &keeper, 0).map_err(|_| "SPAWN_0")?;
            run.registry.await_ready(&a).map_err(|_| "READY_0")?;
            run.start(&root, &image, &a, 0)?;
            run.ready(start + 400_000_000)?;
            let b = run.spawn(&root, &keeper, 1);
            if scenario == "api-publication-fault" {
                if b.as_ref()
                    .err()
                    .is_none_or(|e| e.kind() != io::ErrorKind::AlreadyExists)
                {
                    return Err("EXPECTED_RECEIPT_COLLISION");
                }
            } else {
                let b = b.map_err(|_| "SPAWN_1")?;
                run.registry.await_ready(&b).map_err(|_| "READY_1")?;
                run.start(&root, &image, &b, 1)?;
                run.ready(start + 400_000_000)?;
            }
            let live = run.facts();
            run.event("LIVE_BEFORE_CASE_ACTION", live)?;
            if scenario == "api-edge-fault" {
                let mut refused = Command::new(&image);
                refused
                    .arg("--refuse-extra-start")
                    .env_clear()
                    .current_dir(&root)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if run.registry.start(&a, refused).is_ok() {
                    return Err("REPEAT_START_ACCEPTED");
                }
            }
            if matches!(scenario, "api-edge-fault" | "api-publication-fault") {
                let mut publisher_called = false;
                let attempt = run.registry.spawn_group(&root, &keeper, |_, _| {
                    publisher_called = true;
                    Ok(())
                });
                if attempt.is_ok() || publisher_called || run.registry.report().retained_slots != 2
                {
                    return Err("ADMISSION_REOPENED");
                }
            }
            let requested = if scenario == "api-expired-bound" {
                clock()?.saturating_sub(1)
            } else {
                (clock()? + 600_000_000).min(hard)
            };
            run.registry.begin_cleanup(requested);
            if scenario == "api-clean-two" {
                run.registry.begin_cleanup(requested - 50_000_000);
                run.registry.begin_cleanup(requested + 50_000_000);
                if run.registry.report().cleanup_end != Some(requested - 50_000_000) {
                    return Err("BOUND_EXTENDED");
                }
            }
            let action_end = run.registry.report().cleanup_end.ok_or("NO_CLEANUP_END")?;
            let before = run.facts();
            run.event(
                "BEFORE_FINISH",
                json!({"requested_end":requested,"actual_end":action_end,"facts":before}),
            )?;
            let report = run.registry.finish();
            run.poll_observations()?;
            let at = clock()?;
            if report.retained_slots != 2
                || !report.admission_closed
                || !report.finish_called
                || report.output_eof_proven
            {
                return Err("REGISTRY_FACTS");
            }
            if scenario == "api-expired-bound" {
                if report.first_fault.is_none() || action_end > requested {
                    return Err("EXPIRED_HEALED");
                }
                return Ok(
                    json!({"case_pass":true,"expected_bound_refusal":true,"protocol_outcome":"FAILED",
                "physical_cancel_before_bound":null,"clean_proof":false,"cutpoint_at":at}),
                );
            }
            // finish may precede scheduler delivery of the QA lifeline EOF. This is
            // observation only under the SAME end, never another proof allowance.
            while run.observations.iter().any(|v| v.eof_at.is_none()) && clock()? < action_end {
                run.poll_observations()?;
                nap();
            }
            let at = clock()?;
            if at >= action_end
                || !report.all_direct_waits_known
                || run
                    .observations
                    .iter()
                    .any(|v| v.eof_at.is_none_or(|t| t >= action_end) || v.natural)
            {
                return Err("PHYSICAL_OR_WAIT_UNPROVEN");
            }
            if scenario == "api-clean-two" {
                if report.first_fault.is_some()
                    || !report.all_group_proofs
                    || report.groups.iter().any(|g| {
                        !g.facts.keeper_status.is_some_and(|s| s.success()) || !g.facts.terminal_eof
                    })
                {
                    return Err("CLEAN_UNPROVEN");
                }
            } else if report.first_fault.is_none() {
                return Err("FAILED_OWNER_HEALED");
            }
            Ok(
                json!({"case_pass":true,"protocol_outcome":if scenario=="api-clean-two" {"CLEAN"} else {"FAILED"},
            "physical_cancel_before_bound":true,"clean_proof":scenario=="api-clean-two","cutpoint_at":at}),
            )
        })();
        if let Err(e) = &outcome {
            run.fail(e);
        }
        let first = json!({"scenario":scenario,"binding":bound,"result":outcome.as_ref().ok(),"facts":run.facts()});
        let first_receipt = publish_snapshot(
            &first,
            "FIRST",
            |v| exclusive(&root.join("FIRST.json"), v),
            emit_fact_fallback,
        );
        for e in &first_receipt.errors {
            run.fail(e);
        }
        run.fallback();
        if !run.registry.report().all_direct_waits_known
            || run.observations.iter().any(|o| o.eof_at.is_none())
        {
            run.fail("LATE_UNKNOWN_HANDLES");
        }
        let drift = digest(&image).ok().as_deref() == bound["actor_sha256"].as_str()
            && digest(&keeper).ok().as_deref() == bound["keeper_sha256"].as_str();
        if !drift {
            run.fail("IMAGE_DRIFT");
        }
        let canary = scenario != "api-publication-fault"
            || fs::read(root.join("registration-1.json")).ok().as_deref() == Some(b"CANARY\n");
        if !canary {
            run.fail("CANARY_DRIFT");
        }
        let final_snapshot = json!({"case_pass":outcome.is_ok()&&run.first.is_none(),"cutpoint":&first,
        "first_preserved":first_receipt.durable,"first_fallback_emitted":first_receipt.fallback_emitted,
        "image_drift_free":drift,"collision_canary_unchanged":canary,"facts":run.facts(),
        "late_facts_never_upgrade_first":true});
        let final_receipt = publish_snapshot(
            &final_snapshot,
            "FINAL",
            |v| exclusive(&root.join("FINAL.json"), v),
            emit_fact_fallback,
        );
        for e in &final_receipt.errors {
            run.fail(e);
        }
        if outcome.is_ok() && run.first.is_none() && final_receipt.durable {
            Ok(())
        } else {
            Err("FIRST_FAILED")
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let result = if args.first().is_some_and(|s| s == "--finite-leader") {
            finite_leader(&args[1..])
        } else {
            controller(&args)
        };
        if result.is_err() {
            std::process::exit(1);
        }
    }
}

#[cfg(unix)]
fn main() {
    unix::main();
}
#[cfg(not(unix))]
fn main() {
    std::process::exit(1);
}
