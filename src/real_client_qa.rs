//! Closed, developer-only OFFLINE native version/help discovery.
//!
//! No model, login, trust, consent, transcript or arbitrary command entry exists.
//! External OS policy/launch/artifact review is mandatory before execution.
//! An opaque group's known waits do not establish detached-descendant absence.
use crate::process_ownership::{
    image::VerifiedKeeperImage,
    io::IoPlan,
    protocol::ProtocolError,
    scope::{monotonic_now, Scope, ScopeReport},
};
use serde::Serialize;
use std::{
    ffi::CString,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::Command,
};

const TOTAL_NS: u64 = 4_000_000_000;
const OPERATION_NS: u64 = 1_000_000_000;
const CLEANUP_NS: u64 = 50_000_000;
const ROOT_PARENT: &str = "/Users/sionsmith/.t3/userdata/workspaces/keito-agent-capture-recovery-20261109-8bc862f6/evidence";
const POLICY: &str = "real-client-offline-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Client {
    Claude21319,
    Codex01670Native,
}
impl Client {
    fn path(self) -> &'static str {
        match self {
            Self::Claude21319 => "/Users/sionsmith/.local/share/claude/versions/2.1.319",
            Self::Codex01670Native => "/opt/homebrew/lib/node_modules/@openai/codex/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex",
        }
    }
    fn bytes(self) -> u64 {
        match self {
            Self::Claude21319 => 321_995_200,
            Self::Codex01670Native => 270_815_376,
        }
    }
    fn version(self) -> &'static str {
        match self {
            Self::Claude21319 => "2.1.319",
            Self::Codex01670Native => "0.167.0",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Operation {
    Version,
    RequiredHelp,
}
const PLAN: [(Client, Operation); 4] = [
    (Client::Claude21319, Operation::Version),
    (Client::Claude21319, Operation::RequiredHelp),
    (Client::Codex01670Native, Operation::Version),
    (Client::Codex01670Native, Operation::RequiredHelp),
];
fn arguments(client: Client, operation: Operation) -> &'static [&'static str] {
    match (client, operation) {
        (_, Operation::Version) => &["--version"],
        (Client::Claude21319, Operation::RequiredHelp) => &["--help"],
        (Client::Codex01670Native, Operation::RequiredHelp) => &["exec", "--help"],
    }
}
fn caps(operation: Operation) -> (usize, usize) {
    (
        if operation == Operation::Version {
            8192
        } else {
            65536
        },
        8192,
    )
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Failure {
    Environment,
    PrivateRoot,
    NativePin,
    Pair,
    Clock,
    Admission,
    Operation,
    Version,
    Help,
    Evidence,
    Finality,
}
type Result<T> = std::result::Result<T, Failure>;

struct PlanState {
    end: u64,
    last: u64,
    next: usize,
    failed: bool,
}
impl PlanState {
    fn new(entry: u64, parent: u64) -> Result<Self> {
        let end = entry
            .checked_add(TOTAL_NS)
            .ok_or(Failure::Clock)?
            .min(parent);
        if entry == 0 || end <= entry {
            return Err(Failure::Clock);
        }
        Ok(Self {
            end,
            last: entry,
            next: 0,
            failed: false,
        })
    }
    fn admit(&mut self, at: u64, client: Client, operation: Operation) -> Result<u64> {
        if self.failed
            || at < self.last
            || at >= self.end
            || PLAN.get(self.next) != Some(&(client, operation))
        {
            self.failed = true;
            return Err(Failure::Admission);
        }
        let local = at.checked_add(OPERATION_NS).ok_or(Failure::Clock)?;
        let end = local.min(self.end);
        if at >= end.saturating_sub(CLEANUP_NS) {
            self.failed = true;
            return Err(Failure::Admission);
        }
        self.last = at;
        self.next += 1; // Charge once, before any image/command/spawn operation.
        Ok(end)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
struct Stamp {
    device: u64,
    inode: u64,
    uid: u32,
    mode: u32,
    links: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
// libc's stat integer widths differ across the supported Unix ABIs. Keep
// normalization here; the pre-exec callback performs no allocation or logging.
#[allow(clippy::unnecessary_cast)]
fn stat_matches(m: &libc::stat, expected: Stamp) -> bool {
    m.st_dev as u64 == expected.device
        && m.st_ino as u64 == expected.inode
        && m.st_uid == expected.uid
        && m.st_mode as u32 == expected.mode
        && m.st_nlink as u64 == expected.links
        && m.st_size as u64 == expected.bytes
}
impl Stamp {
    fn of(m: &Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            uid: m.uid(),
            mode: m.mode(),
            links: m.nlink(),
            bytes: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}
struct NativePin {
    file: File,
    stamp: Stamp,
    client: Client,
}
fn native_stamp_allowed(client: Client, stamp: Stamp, uid: u32) -> bool {
    stamp.mode & libc::S_IFMT as u32 == libc::S_IFREG as u32
        && stamp.uid == uid
        && stamp.links == 1
        && stamp.mode & 0o777 == 0o755
        && stamp.bytes == client.bytes()
}
impl NativePin {
    fn open(client: Client) -> Result<Self> {
        let path = Path::new(client.path());
        if path.canonicalize().map_err(|_| Failure::NativePin)? != path {
            return Err(Failure::NativePin);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| Failure::NativePin)?;
        let m = file.metadata().map_err(|_| Failure::NativePin)?;
        if !native_stamp_allowed(client, Stamp::of(&m), unsafe { libc::geteuid() }) {
            return Err(Failure::NativePin);
        }
        let pin = Self {
            file,
            stamp: Stamp::of(&m),
            client,
        };
        pin.check()?;
        Ok(pin)
    }
    fn check(&self) -> Result<()> {
        let held = self.file.metadata().map_err(|_| Failure::NativePin)?;
        let path = fs::symlink_metadata(self.client.path()).map_err(|_| Failure::NativePin)?;
        if Stamp::of(&held) != self.stamp || Stamp::of(&path) != self.stamp {
            return Err(Failure::NativePin);
        }
        Ok(())
    }
}

struct Roots {
    root: PathBuf,
    held: Vec<(PathBuf, File, u64, u64)>,
}
#[cfg(any(target_os = "macos", test))]
fn normalize_descriptors(mut fds: Vec<i32>) -> Result<Vec<i32>> {
    if fds.len() >= 4096 || fds.iter().any(|fd| *fd < 0) {
        return Err(Failure::Admission);
    }
    fds.sort_unstable();
    if fds.windows(2).any(|pair| pair[0] == pair[1]) || [0, 1, 2].iter().any(|fd| !fds.contains(fd))
    {
        return Err(Failure::Admission);
    }
    fds.retain(|fd| *fd >= 3);
    Ok(fds)
}
#[cfg(target_os = "macos")]
fn inherited_descriptors() -> Result<Vec<i32>> {
    // Only this process's numeric FD table, never other processes, descriptor
    // paths or contents. A lowered soft rlimit cannot conceal inherited FDs.
    let mut rows: Vec<libc::proc_fdinfo> = Vec::with_capacity(4096);
    rows.resize_with(4096, || unsafe { std::mem::zeroed() });
    let capacity = std::mem::size_of_val(rows.as_slice());
    let count = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDLISTFDS,
            0,
            rows.as_mut_ptr().cast(),
            capacity as i32,
        )
    };
    if count <= 0
        || count as usize >= capacity
        || !(count as usize).is_multiple_of(std::mem::size_of::<libc::proc_fdinfo>())
    {
        return Err(Failure::Admission);
    }
    rows.truncate(count as usize / std::mem::size_of::<libc::proc_fdinfo>());
    normalize_descriptors(rows.into_iter().map(|row| row.proc_fd).collect())
}
#[cfg(not(target_os = "macos"))]
fn inherited_descriptors() -> Result<Vec<i32>> {
    Err(Failure::Admission)
}
impl Roots {
    fn open(root: PathBuf) -> Result<Self> {
        if root.parent() != Some(Path::new(ROOT_PARENT))
            || !root.file_name().is_some_and(|n| {
                n.to_string_lossy()
                    .starts_with("real-client-offline-first-")
            })
            || root.canonicalize().map_err(|_| Failure::PrivateRoot)? != root
            || std::env::current_dir().map_err(|_| Failure::PrivateRoot)? != root
        {
            return Err(Failure::PrivateRoot);
        }
        let mut held = vec![];
        for name in [
            "",
            "home",
            "home/.claude",
            "home/.codex",
            "tmp",
            "config",
            "state",
            "cache",
            "data",
            "repo",
            "pair",
        ] {
            let path = root.join(name);
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&path)
                .map_err(|_| Failure::PrivateRoot)?;
            let m = file.metadata().map_err(|_| Failure::PrivateRoot)?;
            if m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o777 != 0o700
                || path.canonicalize().map_err(|_| Failure::PrivateRoot)? != path
            {
                return Err(Failure::PrivateRoot);
            }
            held.push((path, file, m.dev(), m.ino()));
        }
        let roots = Self { root, held };
        roots.check()?;
        Ok(roots)
    }
    fn check(&self) -> Result<()> {
        for (path, file, device, inode) in &self.held {
            let a = file.metadata().map_err(|_| Failure::PrivateRoot)?;
            let b = fs::symlink_metadata(path).map_err(|_| Failure::PrivateRoot)?;
            for m in [a, b] {
                if !m.is_dir()
                    || m.dev() != *device
                    || m.ino() != *inode
                    || m.uid() != unsafe { libc::geteuid() }
                    || m.mode() & 0o777 != 0o700
                {
                    return Err(Failure::PrivateRoot);
                }
            }
        }
        Ok(())
    }
    fn command(&self, pin: &NativePin, operation: Operation) -> Result<Command> {
        self.check()?;
        pin.check()?;
        let mut command = Command::new(pin.client.path());
        command
            .args(arguments(pin.client, operation))
            .env_clear()
            .current_dir(self.root.join("repo"));
        for (key, value) in [
            ("HOME", self.root.join("home")),
            ("CODEX_HOME", self.root.join("home/.codex")),
            ("CLAUDE_CONFIG_DIR", self.root.join("home/.claude")),
            ("TMPDIR", self.root.join("tmp")),
            ("TMP", self.root.join("tmp")),
            ("TEMP", self.root.join("tmp")),
            ("XDG_CONFIG_HOME", self.root.join("config")),
            ("XDG_STATE_HOME", self.root.join("state")),
            ("XDG_CACHE_HOME", self.root.join("cache")),
            ("XDG_DATA_HOME", self.root.join("data")),
        ] {
            command.env(key, value);
        }
        command
            .env("PATH", "/usr/bin:/bin")
            .env("LC_CTYPE", "C.UTF-8")
            .env("TERM", "dumb")
            .env("NO_COLOR", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        #[cfg(target_os = "macos")]
        command.env(
            "__CF_USER_TEXT_ENCODING",
            format!("0x{:X}:0:0", unsafe { libc::geteuid() }),
        );
        let path = CString::new(pin.client.path()).map_err(|_| Failure::NativePin)?;
        let expected = pin.stamp;
        let descriptors = inherited_descriptors()?;
        // These callbacks execute before the existing group join/mask restore.
        // Only async-signal-safe syscalls; do not close Rust's exec-error pipe.
        unsafe {
            command.pre_exec(move || {
                let mut m = std::mem::MaybeUninit::<libc::stat>::uninit();
                if libc::lstat(path.as_ptr(), m.as_mut_ptr()) != 0 {
                    return Err(io::Error::from_raw_os_error(libc::ESTALE));
                }
                let m = m.assume_init();
                if !stat_matches(&m, expected) {
                    return Err(io::Error::from_raw_os_error(libc::ESTALE));
                }
                // Subsequent Scope/Command-owned pipes and sockets are created
                // CLOEXEC. Mark the retained entry-thread snapshot as well;
                // this includes inherited descriptors above a lowered rlimit.
                for &fd in &descriptors {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() != Some(libc::EBADF) {
                            return Err(error);
                        }
                    }
                    if flags >= 0 && libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::from_raw_os_error(libc::EBADF));
                    }
                }
                Ok(())
            });
        }
        Ok(command)
    }
}

#[derive(Default, Debug, Serialize)]
struct Capabilities {
    structured_output: bool,
    resume: bool,
    model: bool,
    turn_limit: bool,
    output_token_limit: bool,
    retry_limit: bool,
    session_id: bool,
}
fn project_help(raw: &[u8]) -> Result<Capabilities> {
    let body = std::str::from_utf8(raw).map_err(|_| Failure::Help)?;
    let options: Vec<&str> = body
        .lines()
        .filter(|l| l.trim_start().starts_with('-'))
        .flat_map(str::split_ascii_whitespace)
        .map(|word| word.trim_end_matches(','))
        .collect();
    if options.is_empty() {
        return Err(Failure::Help);
    }
    Ok(Capabilities {
        structured_output: options.contains(&"--json") || options.contains(&"--output-format"),
        resume: options.contains(&"--resume"),
        model: options.contains(&"--model"),
        turn_limit: options.contains(&"--max-turns"),
        output_token_limit: options.contains(&"--max-output-tokens"),
        retry_limit: options.contains(&"--max-retries"),
        session_id: options.contains(&"--session-id"),
    })
}
fn project_version(client: Client, raw: &[u8]) -> Result<()> {
    let body = std::str::from_utf8(raw)
        .map_err(|_| Failure::Version)?
        .trim();
    let expected = match client {
        Client::Claude21319 => "2.1.319 (Claude Code)",
        Client::Codex01670Native => "codex-cli 0.167.0",
    };
    if body == expected {
        Ok(())
    } else {
        Err(Failure::Version)
    }
}
#[derive(Serialize)]
struct Observation {
    client: Client,
    operation: Operation,
    entry: &'static str,
    expected_version: &'static str,
    version_matches: Option<bool>,
    capabilities: Option<Capabilities>,
    started_ns: u64,
    end_ns: u64,
    scope_original_end_ns: u64,
    receipt_finished_ns: Option<u64>,
    exit: Option<i32>,
    signal: Option<i32>,
    waits_known: bool,
    group_absent: bool,
    control_eof: bool,
    stdin_closed: bool,
    stdout_eof: bool,
    stderr_eof: bool,
    stdout_bytes: usize,
    stderr_bytes: usize,
    accepted_ns: Option<u64>,
    native_integrity: &'static str,
    native_metadata: Stamp,
    unregistered_descendants: &'static str,
    product_ack: &'static str,
}
fn write_receipt(root: &Path, name: &str, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| Failure::Evidence)?;
    if bytes.len() > 262_144 {
        return Err(Failure::Evidence);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.join(name))
        .map_err(|_| Failure::Evidence)?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|_| Failure::Evidence)?;
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|_| Failure::Evidence)
}
fn entry_environment(root: &Path) -> Result<()> {
    let mut expected = std::collections::BTreeMap::from([
        ("HOME".to_owned(), root.join("home").into_os_string()),
        ("TMPDIR".to_owned(), root.join("tmp").into_os_string()),
        ("PATH".to_owned(), "/usr/bin:/bin".into()),
        ("LC_CTYPE".to_owned(), "C.UTF-8".into()),
        ("KEITO_REAL_CLIENT_QA_POLICY".to_owned(), POLICY.into()),
    ]);
    #[cfg(target_os = "macos")]
    expected.insert(
        "__CF_USER_TEXT_ENCODING".into(),
        format!("0x{:X}:0:0", unsafe { libc::geteuid() }).into(),
    );
    // Complete attachments are interpreted by Scope, never sent to clients.
    for key in [
        "KEITO_PROCESS_OWNER_FD",
        "KEITO_PROCESS_OWNER_NONCE",
        "KEITO_PROCESS_OWNER_PID",
        "KEITO_PROCESS_OWNER_END_NS",
        "KEITO_PROCESS_OWNER_CONTRACT",
    ] {
        if let Some(value) = std::env::var_os(key) {
            expected.insert(key.into(), value);
        }
    }
    if std::env::vars_os().collect::<std::collections::BTreeMap<_, _>>()
        != expected.into_iter().map(|(k, v)| (k.into(), v)).collect()
    {
        return Err(Failure::Environment);
    }
    Ok(())
}
fn exercise(
    scope: &mut Scope,
    roots: &Roots,
    entry: u64,
    observations: &mut Vec<Observation>,
    claimed: &mut bool,
) -> Result<()> {
    let mut state = PlanState::new(entry, scope.hard_end())?;
    for name in [
        "FIRST.json",
        "FINAL.json",
        "operation-1.json",
        "operation-2.json",
        "operation-3.json",
        "operation-4.json",
    ] {
        match fs::symlink_metadata(roots.root.join(name)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(Failure::Evidence),
        }
    }
    // One durable, exclusive reservation prevents replay/concurrent launches
    // of this root before any pair/native admission or client execution.
    write_receipt(
        &roots.root,
        "START.json",
        &serde_json::json!({
            "schema": 1, "entry_ns": entry, "end_ns": state.end,
            "offline_only": true, "maximum_native_operations": 4,
        }),
    )?;
    *claimed = true;
    let anchor = option_env!("KEITO_RELEASE_GUARD_SHA256").ok_or(Failure::Pair)?;
    let pair = crate::capture_runtime_pair::verify(
        &roots.root.join("pair/keito"),
        anchor,
        env!("CARGO_PKG_VERSION"),
        env!("KEITO_BUILD_TARGET"),
    )
    .map_err(|_| Failure::Pair)?;
    let image = VerifiedKeeperImage::open(&pair.guard_path, &pair.guard_sha256)
        .map_err(|_| Failure::Pair)?;
    for (client, operation) in PLAN {
        scope.poll();
        if scope.failed() {
            return Err(Failure::Operation);
        }
        let started = monotonic_now().map_err(|_| Failure::Clock)?;
        let end = state.admit(started, client, operation)?;
        let native = NativePin::open(client)?;
        let command = roots.command(&native, operation)?;
        let (out, err) = caps(operation);
        let io = IoPlan::new(Vec::new(), out, err).map_err(|_| Failure::Admission)?;
        let slot = scope
            .start_dev_client_opaque_until(command, io, &image, end)
            .map_err(|_| Failure::Admission)?;
        while !scope.slot_complete(&slot) {
            scope.poll();
            if scope.failed() {
                return Err(Failure::Operation);
            }
            std::thread::yield_now();
        }
        let result = scope
            .take_dev_client_result(&slot)
            .map_err(|_| Failure::Finality)?;
        native.check()?;
        roots.check()?;
        let report = scope.report();
        let facts = report.slots.last().ok_or(Failure::Finality)?;
        let mut observation = Observation {
            client,
            operation,
            entry: "DIRECT_NATIVE",
            expected_version: client.version(),
            version_matches: None,
            capabilities: None,
            started_ns: started,
            end_ns: end,
            scope_original_end_ns: state.end,
            receipt_finished_ns: None,
            exit: result.status.code(),
            signal: result.status.signal(),
            waits_known: facts.waits_known,
            group_absent: facts.group_absent,
            control_eof: facts.terminal_eof,
            stdin_closed: facts.io.stdin_closed,
            stdout_eof: facts.io.stdout_eof,
            stderr_eof: facts.io.stderr_eof,
            stdout_bytes: facts.io.stdout_bytes,
            stderr_bytes: facts.io.stderr_bytes,
            accepted_ns: facts.accepted_at,
            native_integrity: "METADATA_AND_VERSION_ONLY",
            native_metadata: native.stamp,
            unregistered_descendants: "UNKNOWN",
            product_ack: "NOT_OBSERVED_OFFLINE",
        };
        let physical = result.status.success()
            && facts.waits_known
            && facts.group_absent
            && facts.terminal_eof
            && facts.io.complete()
            && facts.first_fault.is_none()
            && facts.accepted_at.is_some_and(|at| at < end);
        let projection = if !physical {
            Err(Failure::Finality)
        } else if operation == Operation::Version {
            let checked = project_version(client, &result.output.stdout);
            observation.version_matches = Some(checked.is_ok());
            checked
        } else {
            project_help(&result.output.stdout).map(|value| {
                observation.capabilities = Some(value);
            })
        };
        drop(result); // No raw output/hash in any receipt, error or Debug path.
        observations.push(observation);
        projection?;
        write_receipt(
            &roots.root,
            &format!("operation-{}.json", observations.len()),
            observations.last().ok_or(Failure::Evidence)?,
        )?;
        let durable = monotonic_now().map_err(|_| Failure::Clock)?;
        if durable >= end {
            return Err(Failure::Clock);
        }
        observations
            .last_mut()
            .ok_or(Failure::Evidence)?
            .receipt_finished_ns = Some(durable);
    }
    Ok(())
}

#[derive(Serialize)]
struct Physical {
    slot: usize,
    exit: Option<i32>,
    signal: Option<i32>,
    waits_known: bool,
    group_absent: bool,
    control_eof: bool,
    stdin_closed: bool,
    stdout_eof: bool,
    stderr_eof: bool,
    stdout_bytes: usize,
    stderr_bytes: usize,
    accepted_ns: Option<u64>,
    completion_end_ns: Option<u64>,
    fault: bool,
}
#[derive(Serialize)]
struct Receipt<'a> {
    passed: bool,
    primary: Option<Failure>,
    scope_clean: bool,
    resources_known: bool,
    physical_slots: Vec<Physical>,
    observations: &'a [Observation],
    detached_descendants: &'static str,
    product_qualification: bool,
}
fn receipt<'a>(
    primary: Option<Failure>,
    report: &ScopeReport,
    observations: &'a [Observation],
) -> Receipt<'a> {
    Receipt {
        passed: primary.is_none() && report.clean && observations.len() == 4,
        primary,
        scope_clean: report.clean,
        resources_known: report.all_resources,
        observations,
        physical_slots: report
            .slots
            .iter()
            .map(|slot| Physical {
                slot: slot.slot,
                exit: slot.status.and_then(|s| s.code()),
                signal: slot.status.and_then(|s| s.signal()),
                waits_known: slot.waits_known,
                group_absent: slot.group_absent,
                control_eof: slot.terminal_eof,
                stdin_closed: slot.io.stdin_closed,
                stdout_eof: slot.io.stdout_eof,
                stderr_eof: slot.io.stderr_eof,
                stdout_bytes: slot.io.stdout_bytes,
                stderr_bytes: slot.io.stderr_bytes,
                accepted_ns: slot.accepted_at,
                completion_end_ns: slot.completion_end,
                fault: slot.first_fault.is_some() || slot.io.first_fault.is_some(),
            })
            .collect(),
        detached_descendants: "UNKNOWN",
        product_qualification: false,
    }
}
/// Fixed CLI shape: exactly one fresh private case root. No program/argv/budget input.
pub fn run_offline() -> bool {
    let Ok(entry) = monotonic_now() else {
        return false;
    };
    let Some(end) = entry.checked_add(TOTAL_NS) else {
        return false;
    };
    if !cfg!(target_os = "macos") {
        return false;
    }
    let args: Vec<_> = std::env::args_os().take(4).collect();
    let controls = args.len() == 3 && args[1] == "--check-policy";
    if args.len() != 2 && !controls {
        return false;
    }
    let root = PathBuf::from(&args[if controls { 2 } else { 1 }]);
    if entry_environment(&root).is_err() {
        return false;
    }
    let Ok(mut scope) = Scope::attach_at(entry, end).and_then(|attached| match attached {
        Some(scope) => Ok(scope),
        None => Scope::local_entry(end),
    }) else {
        return false;
    };
    if controls {
        return policy_controls(&mut scope, root, entry, end);
    }
    let mut observations = Vec::with_capacity(4);
    let mut claimed = false;
    let setup = Roots::open(root.clone());
    let mut primary = match setup.as_ref() {
        Ok(roots) => exercise(&mut scope, roots, entry, &mut observations, &mut claimed).err(),
        Err(error) => Some(*error),
    };
    let mut first_attempted = false;
    if primary.is_some() {
        scope.begin_cancel(ProtocolError("DEV_CLIENT_OPERATION"), end);
        if claimed {
            first_attempted = true;
            let _ = write_receipt(
                &root,
                "FIRST.json",
                &receipt(primary, &scope.report(), &observations),
            );
        }
    }
    let report = scope.finish();
    if !report.clean {
        primary.get_or_insert(Failure::Finality);
    }
    if !scope.publish_terminal(primary.is_none()) {
        primary.get_or_insert(Failure::Finality);
    }
    if !first_attempted
        && claimed
        && write_receipt(
            &root,
            "FIRST.json",
            &receipt(primary, &report, &observations),
        )
        .is_err()
    {
        primary.get_or_insert(Failure::Evidence);
    }
    if claimed
        && write_receipt(
            &root,
            "FINAL.json",
            &receipt(primary, &scope.report(), &observations),
        )
        .is_err()
    {
        primary.get_or_insert(Failure::Evidence);
    }
    primary.is_none() && report.clean && observations.len() == 4
}

fn permission_denied(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EPERM) | Some(libc::EACCES))
}
fn policy_controls(scope: &mut Scope, root: PathBuf, entry: u64, end: u64) -> bool {
    let end = end.min(scope.hard_end());
    let Ok(roots) = Roots::open(root) else {
        scope.begin_cancel(ProtocolError("DEV_CLIENT_POLICY_ROOT"), end);
        scope.finish();
        scope.publish_terminal(false);
        return false;
    };
    let canary = Path::new(ROOT_PARENT).join(format!(
        "{}-denied-canary",
        roots.root.file_name().unwrap().to_string_lossy()
    ));
    // The preparer must attest this new run-owned canary exists. Never read its
    // body, even on a policy failure; no original profile/service is probed.
    let file_denied = match File::open(canary) {
        Ok(file) => {
            drop(file);
            false
        }
        Err(error) => permission_denied(&error),
    };
    #[cfg(target_os = "macos")]
    let udp_denied = {
        use std::os::fd::FromRawFd;
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            permission_denied(&io::Error::last_os_error())
        } else {
            // Own the returned socket before any fallible operation. UDP
            // connect does not send a datagram; there is no send/listen call.
            let socket = unsafe { std::net::UdpSocket::from_raw_fd(fd) };
            let denied = socket
                .connect((std::net::Ipv4Addr::LOCALHOST, 9))
                .err()
                .is_some_and(|error| permission_denied(&error));
            drop(socket);
            denied
        }
    };
    #[cfg(not(target_os = "macos"))]
    let udp_denied = false;
    let within_end = monotonic_now().is_ok_and(|at| entry <= at && at < end);
    let operation_clean = file_denied && udp_denied && within_end && roots.check().is_ok();
    if !operation_clean {
        scope.begin_cancel(ProtocolError("DEV_CLIENT_POLICY_DENIAL"), end);
    }
    let report = scope.finish();
    let terminal = scope.publish_terminal(operation_clean && report.clean);
    let receipt = serde_json::json!({
        "kind": "OFFLINE_POLICY_CONTROLS", "file_open_permission_denied": file_denied,
        "udp_endpoint_permission_denied": udp_denied,
        "scope_clean": report.clean, "terminal_clean": terminal,
        "entry_ns": entry, "end_ns": end, "client_invocations": 0,
        "no_datagram_send_or_listener_API": true,
        "original_profile_or_service_probe": false,
        "effective_policy_for_actual_clients": "NOT_YET_QUALIFIED",
    });
    let saved = write_receipt(&roots.root, "POLICY-CONTROLS.json", &receipt).is_ok();
    operation_clean
        && report.clean
        && terminal
        && saved
        && monotonic_now().is_ok_and(|at| entry <= at && at < end)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_client_descriptor_projection_covers_high_fds_and_rejects_ambiguous_tables() {
        assert_eq!(normalize_descriptors(vec![2000, 0, 2, 1]).unwrap(), [2000]);
        assert!(normalize_descriptors(vec![0, 1, 2, 4, 4]).is_err());
        assert!(normalize_descriptors(vec![0, 1, 2, -1]).is_err());
        assert!(normalize_descriptors(vec![1, 2, 4]).is_err());
        assert!(normalize_descriptors(vec![0; 4096]).is_err());
    }
    #[test]
    fn offline_client_offline_plan_has_only_four_fixed_non_model_operations() {
        assert_eq!(PLAN.len(), 4);
        assert_eq!(
            arguments(Client::Claude21319, Operation::Version),
            ["--version"]
        );
        assert_eq!(
            arguments(Client::Claude21319, Operation::RequiredHelp),
            ["--help"]
        );
        assert_eq!(
            arguments(Client::Codex01670Native, Operation::RequiredHelp),
            ["exec", "--help"]
        );
        assert_eq!(
            PLAN.iter()
                .map(|(_, op)| {
                    let (a, b) = caps(*op);
                    a + b
                })
                .sum::<usize>(),
            180_224
        );
    }
    #[test]
    fn offline_client_deadlines_are_not_renewed_and_inherited_parent_wins() {
        let mut state = PlanState::new(100, 2_000_000_100).unwrap();
        assert_eq!(
            state.admit(100, PLAN[0].0, PLAN[0].1).unwrap(),
            1_000_000_100
        );
        assert_eq!(
            state.admit(1_500_000_100, PLAN[1].0, PLAN[1].1).unwrap(),
            2_000_000_100
        );
        assert_eq!(state.end, 2_000_000_100);
        assert!(state.admit(2_000_000_100, PLAN[2].0, PLAN[2].1).is_err());
        assert!(PlanState::new(u64::MAX, u64::MAX).is_err());
    }
    #[test]
    fn offline_client_duplicate_out_of_order_backwards_and_fifth_admission_fail_closed() {
        let mut state = PlanState::new(100, u64::MAX).unwrap();
        assert!(state.admit(100, PLAN[1].0, PLAN[1].1).is_err());
        assert!(state.admit(100, PLAN[0].0, PLAN[0].1).is_err());
        let mut state = PlanState::new(100, u64::MAX).unwrap();
        state.admit(200, PLAN[0].0, PLAN[0].1).unwrap();
        assert!(state.admit(199, PLAN[1].0, PLAN[1].1).is_err());
        let mut state = PlanState::new(100, u64::MAX).unwrap();
        for (client, op) in PLAN {
            state.admit(200, client, op).unwrap();
        }
        assert!(state.admit(200, PLAN[0].0, PLAN[0].1).is_err());
    }
    #[test]
    fn offline_client_version_projection_refuses_near_versions_private_output_and_invalid_utf8() {
        assert!(project_version(Client::Claude21319, b"2.1.319 (Claude Code)\n").is_ok());
        assert!(project_version(Client::Codex01670Native, b"codex-cli 0.167.0\n").is_ok());
        for raw in [
            &b"2.1.322 (Claude Code)"[..],
            &b"PRIVATE_token 0.167.0"[..],
            &[255][..],
        ] {
            assert!(project_version(Client::Codex01670Native, raw).is_err());
        }
    }
    #[test]
    fn offline_client_native_pin_rejects_symlink_mode_owner_link_or_size_substitutes() {
        let valid = Stamp {
            device: 1,
            inode: 2,
            uid: 501,
            mode: libc::S_IFREG as u32 | 0o755,
            links: 1,
            bytes: Client::Claude21319.bytes(),
            modified: (1, 2),
            changed: (3, 4),
        };
        assert!(native_stamp_allowed(Client::Claude21319, valid, 501));
        let mut invalid = valid;
        invalid.links = 2;
        assert!(!native_stamp_allowed(Client::Claude21319, invalid, 501));
        let mut invalid = valid;
        invalid.uid = 502;
        assert!(!native_stamp_allowed(Client::Claude21319, invalid, 501));
        let mut invalid = valid;
        invalid.mode |= 0o020;
        assert!(!native_stamp_allowed(Client::Claude21319, invalid, 501));
        let mut invalid = valid;
        invalid.mode = libc::S_IFLNK as u32 | 0o755;
        assert!(!native_stamp_allowed(Client::Claude21319, invalid, 501));
        let mut invalid = valid;
        invalid.bytes += 1;
        assert!(!native_stamp_allowed(Client::Claude21319, invalid, 501));
    }
    #[test]
    fn offline_client_help_projection_does_not_treat_prose_as_a_capability_or_serialize_it() {
        let value =
            project_help(b"PRIVATE_PROMPT --resume\n  --json  JSON\n  -m, --model <MODEL>\n")
                .unwrap();
        assert!(value.structured_output && value.model && !value.resume);
        let text = serde_json::to_string(&value).unwrap();
        assert!(!text.contains("PRIVATE") && !text.contains("MODEL"));
        assert!(project_help(b"PRIVATE_prose only").is_err());
    }
}
