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

const STDERR_MARKERS: [(&str, &[u8]); 32] = [
    ("BUN_CRASH", b"oh no: Bun has crashed"),
    ("BUN_PANIC", b"panic("),
    ("UNREACHABLE", b"reached unreachable code"),
    ("ASSERTION_FAILED", b"Assertion failed"),
    ("UV_INTERFACE_ADDRESSES", b"uv_interface_addresses"),
    ("UV_CPU_INFO", b"uv_cpu_info"),
    ("UV_USER_INFO", b"uv_os_get_passwd"),
    ("UV_CWD", b"uv_cwd"),
    ("GETIFADDRS", b"getifaddrs"),
    ("GETPWUID", b"getpwuid"),
    ("GETCWD", b"getcwd"),
    ("SYSCTL", b"sysctl"),
    ("MACH_LOOKUP", b"mach-lookup"),
    ("CORE_FOUNDATION", b"CoreFoundation"),
    ("JAVA_SCRIPT_CORE", b"JavaScriptCore"),
    ("OPERATION_NOT_PERMITTED", b"Operation not permitted"),
    ("PERMISSION_DENIED", b"Permission denied"),
    ("ERROR_ACCESS_DENIED", b"AccessDenied"),
    ("ERROR_PERMISSION_DENIED", b"PermissionDenied"),
    ("CODE_EPERM", b"EPERM"),
    ("CODE_EACCES", b"EACCES"),
    ("MEMORY_ALLOCATION", b"OutOfMemory"),
    ("SIGNAL_TRAP", b"Trace/BPT trap"),
    ("BUS_ERROR", b"Bus error"),
    ("MAP_JIT", b"MAP_JIT"),
    ("MMAP", b"mmap"),
    ("MPROTECT", b"mprotect"),
    ("JIT", b"JIT"),
    ("GETRANDOM", b"getrandom"),
    ("THREAD_SUSPEND", b"suspend thread"),
    ("UV_THREAD_CREATE", b"uv_thread_create"),
    ("ERROR_SYSTEM_RESOURCES", b"SystemResources"),
];
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum RuntimePanicKind {
    SegmentationFault,
    IllegalInstruction,
    BusError,
    FloatingPointError,
    UnalignedMemory,
    StackOverflow,
    OutOfMemory,
    Unreachable,
    CppException,
    AbortCalled,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct BunVersion {
    major: u16,
    minor: u16,
    patch: u16,
}
fn runtime_panic_kind(raw: &[u8]) -> Option<RuntimePanicKind> {
    if raw.len() > 8192 {
        return None;
    }
    // Choose before validating. A malformed first candidate cannot authorize
    // attribution from a later fake header in arbitrary diagnostic prose.
    let line = raw
        .split(|byte| *byte == b'\n')
        .find(|line| line.windows(5).any(|part| part == b"panic"))?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.len() > 512 || !line.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return Some(RuntimePanicKind::Unknown);
    }
    let Some(message) = line
        .strip_prefix(b"panic(main thread): ")
        .or_else(|| line.strip_prefix(b"panic: "))
    else {
        return Some(RuntimePanicKind::Unknown);
    };
    for (prefix, kind) in [
        (
            b"Segmentation fault at address 0x".as_slice(),
            RuntimePanicKind::SegmentationFault,
        ),
        (
            b"Illegal instruction at address 0x".as_slice(),
            RuntimePanicKind::IllegalInstruction,
        ),
        (
            b"Bus error at address 0x".as_slice(),
            RuntimePanicKind::BusError,
        ),
        (
            b"Floating point error at address 0x".as_slice(),
            RuntimePanicKind::FloatingPointError,
        ),
    ] {
        if let Some(address) = message.strip_prefix(prefix) {
            return Some(
                if !address.is_empty()
                    && address.len() <= 16
                    && address.iter().all(u8::is_ascii_hexdigit)
                {
                    kind
                } else {
                    RuntimePanicKind::Unknown
                },
            );
        }
    }
    // Only a closed category survives; no exception type/message or address.
    if message.windows(13).any(|part| part == b"C++ exception") {
        return Some(RuntimePanicKind::CppException);
    }
    Some(match message {
        b"Unaligned memory access" => RuntimePanicKind::UnalignedMemory,
        b"Stack overflow" => RuntimePanicKind::StackOverflow,
        b"Bun ran out of memory" => RuntimePanicKind::OutOfMemory,
        b"reached unreachable code" => RuntimePanicKind::Unreachable,
        b"abort() called" => RuntimePanicKind::AbortCalled,
        _ => RuntimePanicKind::Unknown,
    })
}
fn runtime_bun_version(raw: &[u8]) -> Option<BunVersion> {
    if raw.len() > 8192 {
        return None;
    }
    let line = raw
        .split(|byte| *byte == b'\n')
        .find(|line| line.windows(5).any(|part| part == b"Bun v"))?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.len() > 256 || !line.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return None;
    }
    let rest = line.strip_prefix(b"Bun v")?;
    let token = rest.split(u8::is_ascii_whitespace).next()?;
    let mut parts = token.split(|byte| *byte == b'.');
    let mut component = || {
        let value = parts.next()?;
        if value.is_empty()
            || value.len() > 4
            || (value.len() > 1 && value[0] == b'0')
            || !value.iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        std::str::from_utf8(value).ok()?.parse::<u16>().ok()
    };
    let version = BunVersion {
        major: component()?,
        minor: component()?,
        patch: component()?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(version)
}
#[derive(Serialize)]
struct RuntimeDiagnostics {
    within_cap: bool,
    panic_kind: Option<RuntimePanicKind>,
    bun_version: Option<BunVersion>,
    markers: std::collections::BTreeMap<&'static str, bool>,
}
fn runtime_diagnostics(raw: &[u8]) -> RuntimeDiagnostics {
    let within_cap = raw.len() <= 8192;
    RuntimeDiagnostics {
        within_cap,
        panic_kind: runtime_panic_kind(raw),
        bun_version: runtime_bun_version(raw),
        // Only literal keys and booleans survive. Oversize input is not scanned
        // or partially attributed; neither bytes nor a digest enter evidence.
        markers: STDERR_MARKERS
            .iter()
            .map(|(name, needle)| {
                let found = within_cap && raw.windows(needle.len()).any(|part| part == *needle);
                (*name, found)
            })
            .collect(),
    }
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
    Interrupted,
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
    // mode_t is u16 on Darwin and u32 on Linux; widen both without platform casts.
    u64::from(stamp.mode) & u64::from(libc::S_IFMT) == u64::from(libc::S_IFREG)
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
    stderr_diagnostic: RuntimeDiagnostics,
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
            stderr_diagnostic: runtime_diagnostics(&result.output.stderr),
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

// Closed developer diagnostics only. Never serialize a supplied error string.
fn fault_code(reason: Option<&'static str>) -> Option<&'static str> {
    match reason {
        None => None,
        Some(
            code @ ("GROUP_DEADLINE"
            | "GROUP_EOF"
            | "GROUP_READY"
            | "GROUP_PARK"
            | "GROUP_STATE"
            | "GROUP_CLOCK"
            | "GROUP_ADMISSION"
            | "GROUP_START_STATE"
            | "GROUP_SPAWN"
            | "GROUP_KEEPER_FAILED"
            | "GROUP_EARLY_EOF"
            | "GROUP_CLEANUP_DEADLINE"
            | "GROUP_WORKLOAD_WAIT"
            | "GROUP_KEEPER_WAIT"
            | "GROUP_FRAME"
            | "GROUP_NONCE"
            | "GROUP_FRAME_LIMIT"
            | "GROUP_WIRE_LIMIT"
            | "GROUP_TRUNCATED"
            | "GROUP_READ"
            | "GROUP_WRITE"
            | "GROUP_SHORT_WRITE"
            | "GROUP_TERMINAL"
            | "GROUP_SHUTDOWN"
            | "GROUP_BOUND_SEALED"
            | "GROUP_IO"
            | "SCOPE_CLOCK"
            | "SCOPE_EDGE"
            | "SCOPE_BEGIN"
            | "SCOPE_OPERATION_DEADLINE"
            | "SCOPE_CLEANUP_DEADLINE"
            | "SCOPE_RESULT"
            | "SCOPE_IO_ATTACH"
            | "DEV_CLIENT_OPERATION"),
        ) => Some(code),
        Some(_) => Some("UNKNOWN"),
    }
}
#[derive(Serialize)]
struct SetupTiming {
    setup_start_ns: u64,
    ready_end_ns: u64,
    pre_spawn_ns: u64,
    spawn_return_ns: Option<u64>,
    first_ready_poll_ns: Option<u64>,
    last_ready_before_ns: Option<u64>,
    last_ready_after_ns: Option<u64>,
    ready_frame_observed: bool,
    ready_eof_observed: bool,
    deadline_observed: Option<(&'static str, u64)>,
}
impl From<crate::process_ownership::group::SetupTrace> for SetupTiming {
    fn from(trace: crate::process_ownership::group::SetupTrace) -> Self {
        Self {
            setup_start_ns: trace.setup_start_ns,
            ready_end_ns: trace.ready_end_ns,
            pre_spawn_ns: trace.pre_spawn_ns,
            spawn_return_ns: trace.spawn_return_ns,
            first_ready_poll_ns: trace.first_ready_poll_ns,
            last_ready_before_ns: trace.last_ready_before_ns,
            last_ready_after_ns: trace.last_ready_after_ns,
            ready_frame_observed: trace.ready_frame_observed,
            ready_eof_observed: trace.ready_eof_observed,
            deadline_observed: trace.deadline_observed.map(|(point, at)| {
                let point = match point {
                    "READY_BEFORE_RECEIVE" => "READY_BEFORE_RECEIVE",
                    "READY_AFTER_RECEIVE" => "READY_AFTER_RECEIVE",
                    _ => "UNKNOWN",
                };
                (point, at)
            }),
        }
    }
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
    fault_code: Option<&'static str>,
    setup: Option<SetupTiming>,
}
#[derive(Serialize)]
struct Receipt<'a> {
    checks_passed_before_persistence: bool,
    acceptance_requires_exit_zero: bool,
    primary: Option<Failure>,
    scope_fault_code: Option<&'static str>,
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
        checks_passed_before_persistence: primary.is_none()
            && report.clean
            && observations.len() == 4,
        acceptance_requires_exit_zero: true,
        primary,
        scope_fault_code: fault_code(report.first_fault.map(|reason| reason.0)),
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
                fault_code: fault_code(slot.first_fault.or(slot.io.first_fault)),
                setup: slot.setup_trace.map(SetupTiming::from),
            })
            .collect(),
        detached_descendants: "UNKNOWN",
        product_qualification: false,
    }
}

/// This candidate is written after the outcome receipts, but is deliberately
/// not an acceptance assertion: its own persistence and terminal publication
/// must still pass the final clock/cancellation gate, witnessed by exit zero.
#[derive(Serialize)]
struct AcceptanceCandidate {
    kind: &'static str,
    entry_ns: u64,
    end_ns: u64,
    after_outcome_persistence_ns: Option<u64>,
    primary: Option<Failure>,
    acceptance_requires_exit_zero: bool,
    product_qualification: bool,
}
trait TerminalIo {
    fn observe(&mut self) -> Result<u64>;
    fn outcomes(&mut self, primary: Option<Failure>) -> Result<()>;
    fn candidate(&mut self, candidate: &AcceptanceCandidate) -> Result<()>;
    fn publish(&mut self, clean: bool) -> bool;
}
struct EntryTerminal<'a, F> {
    scope: &'a mut Scope,
    root: &'a Path,
    persist_outcomes: F,
}
impl<F: FnMut(&Scope, Option<Failure>) -> Result<()>> TerminalIo for EntryTerminal<'_, F> {
    fn observe(&mut self) -> Result<u64> {
        self.scope.poll();
        if crate::process_ownership::signals::pending() {
            return Err(Failure::Interrupted);
        }
        if self.scope.failed() {
            return Err(Failure::Finality);
        }
        monotonic_now().map_err(|_| Failure::Clock)
    }
    fn outcomes(&mut self, primary: Option<Failure>) -> Result<()> {
        (self.persist_outcomes)(self.scope, primary)
    }
    fn candidate(&mut self, candidate: &AcceptanceCandidate) -> Result<()> {
        write_receipt(self.root, "ACCEPTANCE-CANDIDATE.json", candidate)
    }
    fn publish(&mut self, clean: bool) -> bool {
        self.scope.publish_terminal(clean)
    }
}
fn terminal_checkpoint(
    io: &mut impl TerminalIo,
    entry: u64,
    end: u64,
    last: &mut u64,
    primary: &mut Option<Failure>,
) -> Option<u64> {
    match io.observe().and_then(|at| {
        if at < entry || at < *last || at >= end {
            Err(Failure::Clock)
        } else {
            *last = at;
            Ok(at)
        }
    }) {
        Ok(at) => Some(at),
        Err(error) => {
            primary.get_or_insert(error);
            None
        }
    }
}
fn finalize_terminal(
    io: &mut impl TerminalIo,
    entry: u64,
    end: u64,
    mut primary: Option<Failure>,
) -> bool {
    let mut last = entry;
    terminal_checkpoint(io, entry, end, &mut last, &mut primary);
    if let Err(error) = io.outcomes(primary) {
        primary.get_or_insert(error);
    }
    let after_outcomes = terminal_checkpoint(io, entry, end, &mut last, &mut primary);
    let candidate = AcceptanceCandidate {
        kind: "PROVISIONAL_OFFLINE_ACCEPTANCE",
        entry_ns: entry,
        end_ns: end,
        after_outcome_persistence_ns: after_outcomes,
        primary,
        acceptance_requires_exit_zero: true,
        product_qualification: false,
    };
    if let Err(error) = io.candidate(&candidate) {
        primary.get_or_insert(error);
    }
    // Every durable write is now behind us. A candidate is never accepted
    // solely because it exists: late fsync/cancellation refuses terminal
    // success and exit zero. Preserve the original fault on all later errors.
    terminal_checkpoint(io, entry, end, &mut last, &mut primary);
    if !io.publish(primary.is_none()) {
        primary.get_or_insert(Failure::Finality);
    }
    // Publication can itself observe cancellation or consume the remaining
    // budget. This last observation is the acceptance linearization point;
    // there is no receipt persistence after it.
    terminal_checkpoint(io, entry, end, &mut last, &mut primary);
    primary.is_none()
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
    if !claimed {
        scope.publish_terminal(false);
        return false;
    }
    if observations.len() != 4 {
        primary.get_or_insert(Failure::Finality);
    }
    let end = end.min(scope.hard_end());
    finalize_terminal(
        &mut EntryTerminal {
            scope: &mut scope,
            root: &root,
            persist_outcomes: |scope: &Scope, primary| {
                let report = scope.report();
                let mut failure = None;
                if !first_attempted {
                    failure = write_receipt(
                        &root,
                        "FIRST.json",
                        &receipt(primary, &report, &observations),
                    )
                    .err();
                }
                let final_primary = primary.or(failure);
                if let Err(error) = write_receipt(
                    &root,
                    "FINAL.json",
                    &receipt(final_primary, &report, &observations),
                ) {
                    failure.get_or_insert(error);
                }
                failure.map_or(Ok(()), Err)
            },
        },
        entry,
        end,
        primary,
    )
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
    let primary = if operation_clean && report.clean {
        None
    } else {
        Some(Failure::Finality)
    };
    finalize_terminal(
        &mut EntryTerminal {
            scope,
            root: &roots.root,
            persist_outcomes: |scope: &Scope, primary| {
                let report = scope.report();
                let receipt = serde_json::json!({
                "kind": "OFFLINE_POLICY_CONTROLS", "file_open_permission_denied": file_denied,
                "udp_endpoint_permission_denied": udp_denied,
                "scope_clean": report.clean, "primary": primary,
                "acceptance_requires_exit_zero": true,
                "entry_ns": entry, "end_ns": end, "client_invocations": 0,
                "no_datagram_send_or_listener_API": true,
                "original_profile_or_service_probe": false,
                "effective_policy_for_actual_clients": "NOT_YET_QUALIFIED",
                        });
                write_receipt(&roots.root, "POLICY-CONTROLS.json", &receipt)
            },
        },
        entry,
        end,
        primary,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum During {
        Outcomes,
        Candidate,
        Publication,
    }
    struct FakeTerminal {
        now: u64,
        during: Option<During>,
        advance: u64,
        interrupted: bool,
        interrupt: bool,
        write_failed: bool,
        observations: Vec<&'static str>,
        published: Option<bool>,
        candidate_time: Option<u64>,
        candidate_primary: Option<Failure>,
    }
    impl FakeTerminal {
        fn new(during: Option<During>, advance: u64, interrupt: bool) -> Self {
            Self {
                now: 101,
                during,
                advance,
                interrupted: false,
                interrupt,
                write_failed: false,
                observations: vec![],
                published: None,
                candidate_time: None,
                candidate_primary: None,
            }
        }
        fn step(&mut self, during: During) {
            if self.during == Some(during) {
                self.now = self.advance;
                self.interrupted |= self.interrupt;
            }
        }
    }
    impl TerminalIo for FakeTerminal {
        fn observe(&mut self) -> Result<u64> {
            self.observations.push("observe");
            if self.interrupted {
                Err(Failure::Interrupted)
            } else {
                Ok(self.now)
            }
        }
        fn outcomes(&mut self, _: Option<Failure>) -> Result<()> {
            self.observations.push("outcomes");
            self.step(During::Outcomes);
            if self.write_failed {
                Err(Failure::Evidence)
            } else {
                Ok(())
            }
        }
        fn candidate(&mut self, candidate: &AcceptanceCandidate) -> Result<()> {
            self.observations.push("candidate");
            self.candidate_time = candidate.after_outcome_persistence_ns;
            self.candidate_primary = candidate.primary;
            assert!(candidate.acceptance_requires_exit_zero && !candidate.product_qualification);
            self.step(During::Candidate);
            Ok(())
        }
        fn publish(&mut self, clean: bool) -> bool {
            self.observations.push("publish");
            self.published = Some(clean);
            self.step(During::Publication);
            clean
        }
    }
    #[test]
    fn offline_client_terminal_rejects_deadline_during_outcome_candidate_or_publication() {
        for stage in [During::Outcomes, During::Candidate, During::Publication] {
            for late in [200, 201] {
                let mut io = FakeTerminal::new(Some(stage), late, false);
                assert!(!finalize_terminal(&mut io, 100, 200, None));
            }
        }
    }
    #[test]
    fn offline_client_terminal_rejects_interruption_during_each_persistence_or_publication() {
        for stage in [During::Outcomes, During::Candidate, During::Publication] {
            let mut io = FakeTerminal::new(Some(stage), 102, true);
            assert!(!finalize_terminal(&mut io, 100, 200, None));
        }
    }
    #[test]
    fn offline_client_terminal_witness_and_publication_follow_durable_outcomes() {
        let mut io = FakeTerminal::new(None, 0, false);
        assert!(finalize_terminal(&mut io, 100, 200, None));
        assert_eq!(io.candidate_time, Some(101));
        assert_eq!(
            io.observations,
            [
                "observe",
                "outcomes",
                "observe",
                "candidate",
                "observe",
                "publish",
                "observe"
            ]
        );
        assert_eq!(io.published, Some(true));
    }
    #[test]
    fn offline_client_terminal_rejects_backwards_or_already_expired_inherited_end() {
        let mut io = FakeTerminal::new(Some(During::Outcomes), 100, false);
        assert!(!finalize_terminal(&mut io, 100, 200, None));
        let mut io = FakeTerminal::new(None, 0, false);
        assert!(!finalize_terminal(&mut io, 100, 101, None));
    }
    #[test]
    fn offline_client_terminal_failure_preserves_first_error_and_never_publishes_success() {
        let mut io = FakeTerminal::new(Some(During::Outcomes), 201, true);
        io.write_failed = true;
        assert!(!finalize_terminal(
            &mut io,
            100,
            200,
            Some(Failure::Version)
        ));
        assert_eq!(io.candidate_primary, Some(Failure::Version));
        assert_eq!(io.published, Some(false));
        let mut io = FakeTerminal::new(None, 0, false);
        io.write_failed = true;
        assert!(!finalize_terminal(&mut io, 100, 200, None));
        assert_eq!(io.published, Some(false));
    }
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
            mode: 0o100755, // POSIX regular file with 0755 permissions.
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
        invalid.mode = 0o120755; // POSIX symlink with the same permission bits.
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
    fn diagnostic_report(
        scope_fault: Option<&'static str>,
        slot_fault: Option<&'static str>,
    ) -> ScopeReport {
        ScopeReport {
            first_fault: scope_fault.map(ProtocolError),
            admission_closed: true,
            finish_called: true,
            all_resources: false,
            clean: false,
            slots: vec![crate::process_ownership::scope::SlotReport {
                slot: 0,
                status: None,
                waits_known: false,
                group_absent: false,
                terminal_eof: false,
                io_required: false,
                io: Default::default(),
                accepted_at: None,
                completion_end: Some(80),
                first_fault: slot_fault,
                setup_trace: Some(crate::process_ownership::group::SetupTrace {
                    setup_start_ns: 10,
                    ready_end_ns: 30,
                    pre_spawn_ns: 11,
                    spawn_return_ns: Some(12),
                    first_ready_poll_ns: Some(14),
                    last_ready_before_ns: Some(29),
                    last_ready_after_ns: None,
                    ready_frame_observed: false,
                    ready_eof_observed: true,
                    deadline_observed: Some(("READY_BEFORE_RECEIVE", 31)),
                    fault_origin: None,
                }),
            }],
        }
    }
    #[test]
    fn offline_client_failure_receipt_preserves_first_fault_and_setup_without_output() {
        let report = diagnostic_report(Some("GROUP_DEADLINE"), Some("GROUP_EOF"));
        let value = serde_json::to_value(receipt(Some(Failure::Operation), &report, &[])).unwrap();
        assert_eq!(value["scope_fault_code"], "GROUP_DEADLINE");
        assert_eq!(value["physical_slots"][0]["fault_code"], "GROUP_EOF");
        assert_eq!(value["physical_slots"][0]["setup"]["ready_end_ns"], 30);
        assert_eq!(
            value["physical_slots"][0]["setup"]["ready_eof_observed"],
            true
        );
        assert_eq!(
            value["physical_slots"][0]["setup"]["deadline_observed"][0],
            "READY_BEFORE_RECEIVE"
        );
        assert_eq!(value["primary"], "OPERATION");
        assert_eq!(value["resources_known"], false);
        assert_eq!(value["observations"], serde_json::json!([]));
    }
    #[test]
    fn offline_client_failure_receipt_refuses_unknown_reason_and_phase_text() {
        let mut report =
            diagnostic_report(Some("PRIVATE_SCOPE_CANARY"), Some("PRIVATE_SLOT_CANARY"));
        report.slots[0]
            .setup_trace
            .as_mut()
            .unwrap()
            .deadline_observed = Some(("PRIVATE_PHASE_CANARY", 31));
        let value = serde_json::to_value(receipt(Some(Failure::Operation), &report, &[])).unwrap();
        assert_eq!(value["scope_fault_code"], "UNKNOWN");
        assert_eq!(value["physical_slots"][0]["fault_code"], "UNKNOWN");
        assert_eq!(
            value["physical_slots"][0]["setup"]["deadline_observed"][0],
            "UNKNOWN"
        );
        assert!(!value.to_string().contains("PRIVATE"));
    }
    #[test]
    fn offline_client_failure_receipt_keeps_absent_fault_and_setup_null() {
        let mut report = diagnostic_report(None, None);
        report.slots[0].setup_trace = None;
        let value = serde_json::to_value(receipt(Some(Failure::Finality), &report, &[])).unwrap();
        assert!(value.as_object().unwrap().contains_key("scope_fault_code"));
        assert_eq!(value["scope_fault_code"], serde_json::Value::Null);
        assert!(value["physical_slots"][0]
            .as_object()
            .unwrap()
            .contains_key("fault_code"));
        assert!(value["physical_slots"][0]
            .as_object()
            .unwrap()
            .contains_key("setup"));
        assert_eq!(value["physical_slots"][0]["setup"], serde_json::Value::Null);
    }
    #[test]
    fn offline_client_runtime_diagnostics_classify_only_fixed_public_markers() {
        let value = runtime_diagnostics(
            b"PRIVATE_CANARY uv_cpu_info: EPERM; MAP_JIT mmap; oh no: Bun has crashed",
        );
        assert!(value.within_cap);
        assert!(
            value.markers["UV_CPU_INFO"]
                && value.markers["CODE_EPERM"]
                && value.markers["BUN_CRASH"]
        );
        assert!(!value.markers["UV_INTERFACE_ADDRESSES"]);
        assert!(value.markers["MAP_JIT"] && value.markers["MMAP"] && value.markers["JIT"]);
        let text = serde_json::to_string(&value).unwrap();
        assert!(!text.contains("PRIVATE_CANARY") && !text.contains("uv_cpu_info:"));
    }
    #[test]
    fn offline_client_runtime_diagnostics_unknown_text_has_no_dynamic_keys_or_content() {
        let value = runtime_diagnostics(b"PRIVATE_PROMPT_CANARY /private/PRIVATE_PATH_CANARY 1234");
        assert!(value.within_cap && value.markers.values().all(|x| !x));
        let expected: std::collections::BTreeSet<_> =
            STDERR_MARKERS.iter().map(|(key, _)| *key).collect();
        assert_eq!(
            value
                .markers
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        assert_eq!(expected.len(), 32);
        let text = serde_json::to_string(&value).unwrap();
        assert!(!text.contains("PRIVATE") && !text.contains("1234"));
    }
    #[test]
    fn offline_client_runtime_diagnostics_refuse_oversize_without_matching_prefix() {
        let mut raw = b"oh no: Bun has crashed".to_vec();
        raw.resize(8193, b'x');
        let value = runtime_diagnostics(&raw);
        assert!(!value.within_cap);
        assert!(value.markers.values().all(|x| !x));
    }
    #[test]
    fn offline_client_runtime_diagnostics_empty_is_bounded_without_a_cause() {
        let value = runtime_diagnostics(b"");
        assert!(value.within_cap && value.markers.values().all(|x| !x));
    }
    #[test]
    fn offline_client_panic_kind_projects_public_family_without_address_or_message() {
        assert_eq!(
            runtime_panic_kind(b"panic(main thread): Segmentation fault at address 0xDEADBEEF\n"),
            Some(RuntimePanicKind::SegmentationFault)
        );
        assert_eq!(
            runtime_panic_kind(b"panic(main thread): A C++ exception occurred\n"),
            Some(RuntimePanicKind::CppException)
        );
    }
    #[test]
    fn offline_client_panic_kind_first_malformed_candidate_blocks_later_fake() {
        assert_eq!(
            runtime_panic_kind(
                b"panic(main thread):\npanic(main thread): A C++ exception occurred\n"
            ),
            Some(RuntimePanicKind::Unknown)
        );
        assert_eq!(
            runtime_panic_kind(b"PRIVATE panic hidden\npanic(main thread): Stack overflow\n"),
            Some(RuntimePanicKind::Unknown)
        );
    }
    #[test]
    fn offline_client_panic_kind_private_suffix_or_oversize_is_unknown() {
        assert_eq!(
            runtime_panic_kind(
                b"panic(main thread): Segmentation fault at address 0x1 PRIVATE_PATH\n"
            ),
            Some(RuntimePanicKind::Unknown)
        );
        let mut raw = b"panic(main thread): A C++ exception occurred\n".to_vec();
        raw.resize(8193, b'x');
        assert_eq!(runtime_panic_kind(&raw), None);
    }
    #[test]
    fn offline_client_bun_version_projects_numeric_components_only() {
        assert_eq!(
            runtime_bun_version(b"Bun v1.3.0 (PRIVATE_HASH) macOS Silicon\n"),
            Some(BunVersion {
                major: 1,
                minor: 3,
                patch: 0
            })
        );
        let text =
            serde_json::to_string(&runtime_bun_version(b"Bun v1.3.0 (PRIVATE_HASH)\n")).unwrap();
        assert!(!text.contains("PRIVATE") && !text.contains("macOS"));
    }
    #[test]
    fn offline_client_bun_version_first_invalid_candidate_cannot_fall_through() {
        assert_eq!(
            runtime_bun_version(b"Bun vPRIVATE\nBun v1.3.0 (fake)\n"),
            None
        );
        assert_eq!(
            runtime_bun_version(b"PRIVATE Bun v1.3.0\nBun v1.3.0 (fake)\n"),
            None
        );
        assert_eq!(runtime_bun_version(b"Bun v1.3.0PRIVATE\n"), None);
    }
    #[test]
    fn offline_client_bun_version_rejects_overflow_non_ascii_empty_and_oversize() {
        for raw in [
            b"Bun v65536.3.0".as_slice(),
            b"Bun v1.03.0",
            b"Bun v1.3.0\xff",
            b"",
        ] {
            assert_eq!(runtime_bun_version(raw), None);
        }
        let mut raw = b"Bun v1.3.0\n".to_vec();
        raw.resize(8193, b'x');
        assert_eq!(runtime_bun_version(&raw), None);
    }
}
