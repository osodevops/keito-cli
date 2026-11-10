//! Development-only reserved group cleanup. Not connected to product commands.
//!
//! The keeper is a sibling, never a wrapper/ancestor of the workload. Only the
//! keeper queries or signals its own reserved PGID. An opaque same-session tool
//! may use this lease; the guardian's setsid core must be a cooperative owner.
use super::protocol::{ProtocolError, SharedOwner};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{fs::MetadataExt, net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::Duration,
};

const MAX_FRAME: usize = 256;
const MAX_WIRE: usize = 4096;
const CLEANUP_NS: u64 = 1_800_000_000;
const TERM_NS: u64 = 15_000_000;
const MAX_LIFETIME_NS: u64 = 900_000_000_000;

/// Closed policies, not caller-supplied timeout extensions. The short policy
/// must fit inside an inherited product end; it is not a latency qualification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupPolicy {
    Legacy,
    HookShort,
}
impl CleanupPolicy {
    fn reserve_ns(self) -> u64 {
        match self {
            Self::Legacy => CLEANUP_NS,
            Self::HookShort => 50_000_000,
        }
    }
    fn term_ns(self) -> u64 {
        TERM_NS
    }
    fn setup_ns(self) -> u64 {
        match self {
            Self::Legacy => CLEANUP_NS,
            Self::HookShort => 20_000_000,
        }
    }
    fn keeper_tag(tag: Option<&str>) -> io::Result<Self> {
        match tag {
            None => Ok(Self::Legacy),
            Some("hook-short-v1") => Ok(Self::HookShort),
            _ => Err(refused("KEEPER_POLICY")),
        }
    }
    fn setup_end(self, at: u64, hard_end: u64) -> u64 {
        at.saturating_add(self.setup_ns())
            .min(hard_end.saturating_sub(self.reserve_ns()))
    }
    fn observation_end(self, at: u64, inherited_end: u64) -> u64 {
        at.saturating_add(self.reserve_ns()).min(inherited_end)
    }
}

fn refused(reason: &'static str) -> io::Error {
    io::Error::other(reason)
}
fn now() -> io::Result<u64> {
    let mut clock = std::mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, clock.as_mut_ptr()) } != 0 {
        return Err(refused("GROUP_CLOCK"));
    }
    let clock = unsafe { clock.assume_init() };
    u64::try_from(clock.tv_sec)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .and_then(|seconds| seconds.checked_add(clock.tv_nsec as u64))
        .ok_or_else(|| refused("GROUP_CLOCK"))
}
fn number(value: &str) -> io::Result<u64> {
    if value.is_empty()
        || value.len() > 20
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|v| v.is_ascii_digit())
    {
        return Err(refused("GROUP_INTEGER"));
    }
    value.parse().map_err(|_| refused("GROUP_INTEGER"))
}
fn nonce_valid(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn pause() {
    std::thread::sleep(Duration::from_millis(1));
}

/// No cached PID is a signalling capability. This machine permits GONE only
/// while the keeper reservation is held and forbids all operations afterward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AwaitReady,
    Ready,
    Joined,
    Parked,
    Cleaning,
    Gone,
    Released,
    Failed,
}
struct Progress {
    phase: Phase,
    first_fault: Option<&'static str>,
    deadline_ns: u64,
    secondary_faults: Vec<&'static str>,
}
impl Progress {
    fn new(deadline_ns: u64) -> Self {
        Self {
            phase: Phase::AwaitReady,
            first_fault: None,
            deadline_ns,
            secondary_faults: Vec::new(),
        }
    }
    fn advance(&mut self, from: Phase, to: Phase) -> io::Result<()> {
        let valid = matches!(
            (from, to),
            (Phase::AwaitReady, Phase::Ready)
                | (Phase::Ready, Phase::Joined)
                | (Phase::Joined, Phase::Parked)
                | (Phase::Parked, Phase::Cleaning)
                | (Phase::Cleaning, Phase::Gone)
                | (Phase::Gone, Phase::Released)
        );
        if self.first_fault.is_some() || self.phase != from || !valid {
            self.fail("GROUP_STATE");
            return Err(refused(self.first_fault.unwrap()));
        }
        self.phase = to;
        Ok(())
    }
    fn begin_cleanup(&mut self) -> io::Result<()> {
        match self.phase {
            Phase::AwaitReady | Phase::Ready | Phase::Joined | Phase::Parked => {
                self.phase = Phase::Cleaning;
                Ok(())
            }
            Phase::Cleaning | Phase::Gone => Ok(()),
            _ => Err(refused("GROUP_STATE")),
        }
    }
    fn shorten(&mut self, deadline_ns: u64) {
        self.deadline_ns = self.deadline_ns.min(deadline_ns);
    }
    fn fail(&mut self, reason: &'static str) {
        if let Some(first) = self.first_fault {
            if reason != first
                && !self.secondary_faults.contains(&reason)
                && self.secondary_faults.len() < 8
            {
                self.secondary_faults.push(reason);
            }
        } else {
            self.first_fault = Some(reason);
        }
        self.phase = Phase::Failed;
    }
    #[cfg(test)]
    fn may_touch_group(&self) -> bool {
        matches!(self.phase, Phase::Parked | Phase::Cleaning) && self.first_fault.is_none()
    }
}

/// No complete frame is different from a drained transport. A budget yield
/// must not authorize the keeper's next query/signal step.
enum Receive {
    Frame(String),
    // A whole rejected line was consumed; its known boundary permits bounded
    // failure-only draining. This is NOT evidence of transport quiescence.
    Rejected(&'static str),
    Quiescent,
    Eof,
    BudgetYield,
}

/// Separate closed keeper protocol. No paths/content/auth/capture fields.
struct Channel<S = UnixStream> {
    stream: S,
    nonce: String,
    pending: Vec<u8>,
    wire: usize,
    eof: bool,
}
impl Channel {
    fn new(stream: UnixStream, nonce: String) -> io::Result<Self> {
        if !nonce_valid(&nonce) {
            return Err(refused("GROUP_NONCE"));
        }
        stream.set_nonblocking(true)?;
        if unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(refused("GROUP_FD"));
        }
        Ok(Self {
            stream,
            nonce,
            pending: Vec::new(),
            wire: 0,
            eof: false,
        })
    }
}
impl<S: Read + Write> Channel<S> {
    fn charge(&mut self, bytes: usize) -> io::Result<()> {
        self.wire = self
            .wire
            .checked_add(bytes)
            .filter(|n| *n <= MAX_WIRE)
            .ok_or_else(|| refused("GROUP_WIRE_LIMIT"))?;
        Ok(())
    }
    fn send(&mut self, body: &str) -> io::Result<()> {
        if !body.is_ascii()
            || body.contains(['\n', '\r'])
            || body.len() + 34 > MAX_FRAME
            || self.eof
        {
            return Err(refused("GROUP_FRAME"));
        }
        let bytes = format!("{}|{}\n", self.nonce, body);
        self.charge(bytes.len())?;
        if self
            .stream
            .write(bytes.as_bytes())
            .map_err(|_| refused("GROUP_WRITE"))?
            != bytes.len()
        {
            return Err(refused("GROUP_SHORT_WRITE"));
        }
        Ok(())
    }
    fn receive_event(&mut self) -> io::Result<Receive> {
        for _ in 0..8 {
            if let Some(end) = self.pending.iter().position(|v| *v == b'\n') {
                let line: Vec<_> = self.pending.drain(..=end).collect();
                if line.len() > MAX_FRAME || !line.is_ascii() {
                    return Ok(Receive::Rejected("GROUP_FRAME"));
                }
                let Ok(text) = std::str::from_utf8(&line[..line.len() - 1]) else {
                    return Ok(Receive::Rejected("GROUP_FRAME"));
                };
                let Some((nonce, body)) = text.split_once('|') else {
                    return Ok(Receive::Rejected("GROUP_FRAME"));
                };
                if nonce != self.nonce || body.contains('\r') {
                    return Ok(Receive::Rejected("GROUP_NONCE"));
                }
                return Ok(Receive::Frame(body.to_owned()));
            }
            if self.pending.len() >= MAX_FRAME {
                return Err(refused("GROUP_FRAME_LIMIT"));
            }
            if self.eof {
                return Ok(Receive::Eof);
            }
            let mut bytes = [0u8; MAX_FRAME];
            match self.stream.read(&mut bytes) {
                Ok(0) => {
                    self.eof = true;
                    if !self.pending.is_empty() {
                        return Err(refused("GROUP_TRUNCATED"));
                    }
                    return Ok(Receive::Eof);
                }
                Ok(n) => {
                    self.charge(n)?;
                    self.pending.extend_from_slice(&bytes[..n]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(if self.pending.is_empty() {
                        Receive::Quiescent
                    } else {
                        Receive::BudgetYield
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(refused("GROUP_READ")),
            }
        }
        Ok(Receive::BudgetYield)
    }
    // Parent frame polling performs no group operations. Its EOF predicate is
    // still separate; a budget yield cannot satisfy terminal+EOF proof.
    fn receive(&mut self) -> io::Result<Option<String>> {
        match self.receive_event()? {
            Receive::Frame(body) => Ok(Some(body)),
            Receive::Rejected(reason) => Err(refused(reason)),
            Receive::Quiescent | Receive::Eof | Receive::BudgetYield => Ok(None),
        }
    }
    fn wait_frame(&mut self, deadline: u64) -> io::Result<String> {
        while now()? < deadline {
            if let Some(frame) = self.receive()? {
                return Ok(frame);
            }
            if self.eof {
                return Err(refused("GROUP_EOF"));
            }
            pause();
        }
        Err(refused("GROUP_DEADLINE"))
    }
}

/// Cleanup is reserved INSIDE the admitted lifetime, never added after expiry.
#[derive(Clone, Copy)]
struct Lifetime {
    operation_end: u64,
    hard_end: u64,
    policy: CleanupPolicy,
}
impl Lifetime {
    fn new(at: u64, hard_end: u64) -> io::Result<Self> {
        Self::with_policy(at, hard_end, CleanupPolicy::Legacy)
    }
    fn with_policy(at: u64, hard_end: u64, policy: CleanupPolicy) -> io::Result<Self> {
        let remaining = hard_end
            .checked_sub(at)
            .ok_or_else(|| refused("GROUP_LIFETIME"))?;
        if remaining <= policy.reserve_ns() || remaining > MAX_LIFETIME_NS {
            return Err(refused("GROUP_LIFETIME"));
        }
        Ok(Self {
            operation_end: hard_end - policy.reserve_ns(),
            hard_end,
            policy,
        })
    }
}

/// Returned even on failure by report(); waits do not rehabilitate clean proof.
#[derive(Clone, Debug)]
pub struct GroupReport {
    /// Known direct handles only; these numbers are metadata, never authority
    /// for a caller to query or signal a cached PID/group.
    pub keeper_pid: u32,
    pub workload_pid: Option<u32>,
    pub first_fault: Option<&'static str>,
    pub secondary_faults: Vec<&'static str>,
    pub workload_spawned: bool,
    pub workload_status: Option<ExitStatus>,
    pub keeper_status: Option<ExitStatus>,
    pub group_absent: bool,
    pub release_sent: bool,
    pub terminal_eof: bool,
}
/// Actual direct wait facts, separate from the keeper's group-absence evidence.
#[derive(Debug)]
pub struct GroupProof {
    pub workload_status: Option<ExitStatus>,
    pub keeper_status: ExitStatus,
    pub group_absent: bool,
}

// The same wait/protocol driver is exercised by finite fake handles; no child
// or group syscall is needed to test faulted/expired explicit finalization.
trait DirectWait {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
}
impl DirectWait for Child {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
}
struct Settlement {
    progress: Progress,
    workload_status: Option<ExitStatus>,
    keeper_status: Option<ExitStatus>,
    cleanup_sent: bool,
    release_sent: bool,
    group_absent: bool,
    expected_ready: Option<String>,
    policy: CleanupPolicy,
}
impl Settlement {
    fn new(hard_end: u64) -> Self {
        Self::with_policy(hard_end, CleanupPolicy::Legacy)
    }
    fn with_policy(hard_end: u64, policy: CleanupPolicy) -> Self {
        Self {
            progress: Progress::new(hard_end),
            workload_status: None,
            keeper_status: None,
            cleanup_sent: false,
            release_sent: false,
            group_absent: false,
            expected_ready: None,
            policy,
        }
    }
    fn cap_failed_observation(&mut self, at: Option<u64>, requested: u64) {
        // No usable clock means no newly granted observation interval.
        let end = at
            .map(|at| self.policy.observation_end(at, requested))
            .unwrap_or(0);
        self.progress.shorten(end);
    }
    fn begin(&mut self, deadline_ns: u64, at: u64, channel: &mut Channel) -> io::Result<()> {
        if let Some(first) = self.progress.first_fault {
            return Err(refused(first));
        }
        // RELEASE+write EOF seals the last shared bound. A later reduction
        // is explicitly refused (never silently accepted only in parent).
        if self.release_sent {
            return if deadline_ns < self.progress.deadline_ns {
                Err(refused("GROUP_BOUND_SEALED"))
            } else {
                Ok(())
            };
        }
        let end = deadline_ns
            .min(at.saturating_add(self.policy.reserve_ns()))
            .min(self.progress.deadline_ns);
        if !self.cleanup_sent || end < self.progress.deadline_ns {
            channel.send(&format!("CLEAN|{end}"))?;
            self.progress.shorten(end);
        }
        self.cleanup_sent = true;
        // Includes explicit pre-READY cancellation. The queued READY must
        // still be parsed and matched before a GONE can be accepted.
        self.progress.begin_cleanup()?;
        Ok(())
    }
    fn collect(&mut self, keeper: &mut impl DirectWait, workload: Option<&mut impl DirectWait>) {
        // Both known handles get an attempt, even after first failure/deadline.
        if self.workload_status.is_none() {
            if let Some(child) = workload {
                match child.try_wait() {
                    Ok(status) => self.workload_status = status,
                    Err(_) => self.progress.fail("GROUP_WORKLOAD_WAIT"),
                }
            }
        }
        if self.keeper_status.is_none() {
            match keeper.try_wait() {
                Ok(status) => self.keeper_status = status,
                Err(_) => self.progress.fail("GROUP_KEEPER_WAIT"),
            }
        }
    }
    fn poll(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<u64>,
        pid: libc::pid_t,
        channel: &mut Channel,
        keeper: &mut impl DirectWait,
        workload: Option<&mut impl DirectWait>,
    ) -> io::Result<Option<GroupProof>> {
        let has_workload = workload.is_some();
        self.collect(keeper, workload);
        if self.progress.phase == Phase::Released && self.progress.first_fault.is_none() {
            return Ok(Some(GroupProof {
                workload_status: self.workload_status,
                keeper_status: self.keeper_status.unwrap(),
                group_absent: true,
            }));
        }
        let at = match clock() {
            Ok(at) => at,
            Err(_) => {
                self.progress.fail("GROUP_CLOCK");
                return Err(refused(self.progress.first_fault.unwrap()));
            }
        };
        if !self.cleanup_sent {
            self.progress.fail("GROUP_CLEANUP_NOT_STARTED");
        }
        if at >= self.progress.deadline_ns {
            self.progress.fail("GROUP_CLEANUP_DEADLINE");
        }
        if let Some(reason) = self.progress.first_fault {
            return Err(refused(reason));
        }
        let outcome = (|| {
            let body = channel.receive()?;
            if clock()? >= self.progress.deadline_ns {
                return Err(refused("GROUP_CLEANUP_DEADLINE"));
            }
            if let Some(body) = body {
                if let Some(expected) = &self.expected_ready {
                    if body != *expected {
                        return Err(refused("GROUP_READY"));
                    }
                    self.expected_ready = None;
                } else {
                    if body != format!("GONE|{pid}") || self.progress.phase != Phase::Cleaning {
                        return Err(refused("GROUP_TERMINAL"));
                    }
                    self.progress.advance(Phase::Cleaning, Phase::Gone)?;
                    self.group_absent = true;
                }
            }
            if channel.eof && !self.release_sent {
                return Err(refused("GROUP_EARLY_EOF"));
            }
            if self.progress.phase == Phase::Gone
                && (!has_workload || self.workload_status.is_some())
                && !self.release_sent
            {
                if clock()? >= self.progress.deadline_ns {
                    return Err(refused("GROUP_CLEANUP_DEADLINE"));
                }
                channel.send("RELEASE")?;
                channel
                    .stream
                    .shutdown(std::net::Shutdown::Write)
                    .map_err(|_| refused("GROUP_SHUTDOWN"))?;
                self.release_sent = true;
            }
            if let Some(status) = self.keeper_status {
                if !status.success() {
                    return Err(refused("GROUP_KEEPER_FAILED"));
                }
                if channel.eof && channel.pending.is_empty() && self.release_sent {
                    if clock()? >= self.progress.deadline_ns {
                        return Err(refused("GROUP_CLEANUP_DEADLINE"));
                    }
                    self.progress.advance(Phase::Gone, Phase::Released)?;
                    return Ok(Some(GroupProof {
                        workload_status: self.workload_status,
                        keeper_status: status,
                        group_absent: true,
                    }));
                }
            }
            Ok(None)
        })();
        if let Err(error) = &outcome {
            self.progress.fail(safe_reason(error));
        }
        outcome
    }
}
// Only literals generated here enter durable diagnostics; never raw OS errors.
fn safe_reason(error: &io::Error) -> &'static str {
    match error.to_string().as_str() {
        "GROUP_CLEANUP_DEADLINE" => "GROUP_CLEANUP_DEADLINE",
        "GROUP_NONCE" => "GROUP_NONCE",
        "GROUP_FRAME" => "GROUP_FRAME",
        "GROUP_FRAME_LIMIT" => "GROUP_FRAME_LIMIT",
        "GROUP_WIRE_LIMIT" => "GROUP_WIRE_LIMIT",
        "GROUP_TRUNCATED" => "GROUP_TRUNCATED",
        "GROUP_READ" => "GROUP_READ",
        "GROUP_WRITE" => "GROUP_WRITE",
        "GROUP_SHORT_WRITE" => "GROUP_SHORT_WRITE",
        "GROUP_TERMINAL" => "GROUP_TERMINAL",
        "GROUP_SHUTDOWN" => "GROUP_SHUTDOWN",
        "GROUP_EARLY_EOF" => "GROUP_EARLY_EOF",
        "GROUP_KEEPER_FAILED" => "GROUP_KEEPER_FAILED",
        "GROUP_EOF" => "GROUP_EOF",
        "GROUP_DEADLINE" => "GROUP_DEADLINE",
        "GROUP_CLOCK" => "GROUP_CLOCK",
        "GROUP_STATE" => "GROUP_STATE",
        "GROUP_ADMISSION" => "GROUP_ADMISSION",
        "GROUP_READY" => "GROUP_READY",
        "GROUP_PARK" => "GROUP_PARK",
        "GROUP_SPAWN" => "GROUP_SPAWN",
        "GROUP_START_STATE" => "GROUP_START_STATE",
        "GROUP_BOUND_SEALED" => "GROUP_BOUND_SEALED",
        _ => "GROUP_IO",
    }
}

/// Register immediately after spawn_keeper, BEFORE await_ready/start/receipts.
/// Failed objects retain both handles and expose actual wait facts via report.
pub struct GroupLease {
    keeper: Child,
    workload: Option<Child>,
    settlement: Settlement,
    channel: Channel,
    owner: SharedOwner,
    keeper_pid: libc::pid_t,
    owner_pgid: libc::pid_t,
    sid: libc::pid_t,
}
impl GroupLease {
    pub fn spawn_keeper(
        owner: SharedOwner,
        run_root: &Path,
        keeper_image: &Path,
        lifetime_deadline_ns: u64,
    ) -> io::Result<Self> {
        Self::spawn_keeper_with_policy(
            owner,
            run_root,
            keeper_image,
            lifetime_deadline_ns,
            CleanupPolicy::Legacy,
        )
    }
    /// Explicit short policy for same-session leaves. Product image authority,
    /// cooperative core and I/O wiring remain separate prerequisites.
    pub fn spawn_keeper_with_policy(
        owner: SharedOwner,
        run_root: &Path,
        keeper_image: &Path,
        lifetime_deadline_ns: u64,
        policy: CleanupPolicy,
    ) -> io::Result<Self> {
        owner
            .borrow()
            .check_admission()
            .map_err(|_| refused("GROUP_ADMISSION"))?;
        let current = now()?;
        if Lifetime::with_policy(current, lifetime_deadline_ns, policy).is_err() {
            return Err(refused("GROUP_LIFETIME"));
        }
        for path in [run_root, keeper_image] {
            if !path.is_absolute() || path.canonicalize().ok().as_deref() != Some(path) {
                return Err(refused("GROUP_OWNED_PATH"));
            }
        }
        let root = fs::symlink_metadata(run_root)?;
        let image = fs::symlink_metadata(keeper_image)?;
        if !root.is_dir()
            || root.uid() != unsafe { libc::geteuid() }
            || root.mode() & 0o777 != 0o700
            || !keeper_image.starts_with(run_root)
            || !image.is_file()
            || image.file_type().is_symlink()
            || image.uid() != root.uid()
            || image.nlink() != 1
            || image.mode() & 0o777 != 0o700
        {
            return Err(refused("GROUP_OWNED_PATH"));
        }
        let mut random = [0u8; 16];
        if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
            return Err(refused("GROUP_RANDOM"));
        }
        let nonce: String = random.iter().map(|v| format!("{v:02x}")).collect();
        let (parent, child_end) = UnixStream::pair()?;
        let inherited = unsafe { libc::fcntl(child_end.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 4) };
        if inherited < 0 {
            return Err(refused("GROUP_FD"));
        }
        let inherited = unsafe { OwnedFd::from_raw_fd(inherited) };
        let child_fd = inherited.as_raw_fd();
        let channel = Channel::new(parent, nonce.clone())?;
        let owner_pid = std::process::id();
        let owner_pgid = unsafe { libc::getpgrp() };
        let sid = unsafe { libc::getsid(0) };
        let mut command = Command::new(keeper_image);
        command
            .env_clear()
            .current_dir(run_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .args([
                child_fd.to_string(),
                nonce,
                owner_pid.to_string(),
                owner_pgid.to_string(),
                sid.to_string(),
                lifetime_deadline_ns.to_string(),
            ]);
        if policy == CleanupPolicy::HookShort {
            command.arg("hook-short-v1");
        }
        // Core Foundation may initialize before this helper's main. Supply
        // synthetic UID/zero encoding metadata instead of consulting the
        // operator's encoding file. No auth/profile value is inherited.
        #[cfg(target_os = "macos")]
        command.env(
            "__CF_USER_TEXT_ENCODING",
            format!("0x{:X}:0:0", unsafe { libc::getuid() }),
        );
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 || libc::fcntl(child_fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let keeper = command.spawn()?;
        let keeper_pid = keeper.id() as libc::pid_t;
        // The real handle is owned before any fallible read or publication.
        let mut settlement = if policy == CleanupPolicy::Legacy {
            Settlement::new(lifetime_deadline_ns)
        } else {
            Settlement::with_policy(lifetime_deadline_ns, policy)
        };
        settlement.expected_ready = Some(format!("READY|{keeper_pid}|{keeper_pid}|{sid}"));
        let lease = Self {
            keeper,
            workload: None,
            settlement,
            channel,
            owner,
            keeper_pid,
            owner_pgid,
            sid,
        };
        drop(inherited);
        drop(child_end);
        // Return BEFORE READY I/O. The caller registers even a failing
        // preparation in its owned registry before calling await_ready.
        Ok(lease)
    }

    fn fail(&mut self, reason: &'static str) {
        self.settlement.progress.fail(reason);
        let first = self.settlement.progress.first_fault.unwrap();
        self.owner.borrow_mut().poison(ProtocolError(first));
        if let Some(child) = &mut self.workload {
            child.stdin.take();
        }
        // EOF requests failure-only keeper cleanup even when CLEAN cannot be
        // sent. No parent group authority is acquired by this failure path.
        if self
            .channel
            .stream
            .shutdown(std::net::Shutdown::Write)
            .is_err()
        {
            self.settlement.progress.fail("GROUP_SHUTDOWN");
        }
    }
    fn checked<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if let Err(error) = &result {
            self.fail(safe_reason(error));
        }
        result
    }
    fn check_admission(&mut self) -> io::Result<()> {
        if self.owner.borrow().check_admission().is_err() {
            self.fail("GROUP_ADMISSION");
            return Err(refused("GROUP_ADMISSION"));
        }
        Ok(())
    }
    pub fn await_ready(&mut self) -> io::Result<()> {
        let result = (|| {
            self.check_admission()?;
            if self.settlement.progress.phase != Phase::AwaitReady {
                return Err(refused("GROUP_STATE"));
            }
            let deadline = self
                .settlement
                .policy
                .setup_end(now()?, self.settlement.progress.deadline_ns);
            let body = self.channel.wait_frame(deadline)?;
            if body != format!("READY|{}|{}|{}", self.keeper_pid, self.keeper_pid, self.sid) {
                return Err(refused("GROUP_READY"));
            }
            self.settlement.expected_ready = None;
            self.settlement
                .progress
                .advance(Phase::AwaitReady, Phase::Ready)?;
            self.check_admission()
        })();
        self.checked(result)
    }
    /// Consume the ORIGINAL prepared Command. Its join callback can never escape
    /// back to a caller for reuse after release. No argv/env/FD reconstruction.
    pub fn start(&mut self, mut command: Command) -> io::Result<()> {
        let result = (|| {
            self.check_admission()?;
            if self.settlement.progress.phase != Phase::Ready || self.workload.is_some() {
                return Err(refused("GROUP_START_STATE"));
            }
            if now()?
                >= self
                    .settlement
                    .progress
                    .deadline_ns
                    .saturating_sub(self.settlement.policy.reserve_ns())
            {
                return Err(refused("GROUP_DEADLINE"));
            }
            let pgid = self.keeper_pid;
            let sid = self.sid;
            unsafe {
                command.pre_exec(move || {
                    if libc::getsid(0) != sid || libc::setpgid(0, pgid) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn().map_err(|_| refused("GROUP_SPAWN"))?;
            self.workload = Some(child);
            self.settlement
                .progress
                .advance(Phase::Ready, Phase::Joined)?;
            self.check_admission()?;
            self.channel.send("PARK")?;
            let response = self.channel.wait_frame(
                self.settlement
                    .policy
                    .setup_end(now()?, self.settlement.progress.deadline_ns),
            )?;
            if response
                != format!(
                    "PARKED|{}|{}|{}",
                    self.keeper_pid, self.owner_pgid, self.sid
                )
            {
                return Err(refused("GROUP_PARK"));
            }
            self.settlement
                .progress
                .advance(Phase::Joined, Phase::Parked)
        })();
        self.checked(result)
    }
    /// Transferred pipes MUST have a caller-scoped close/drain/EOF owner. Neither
    /// GroupProof nor terminal keeper EOF proves those output streams drained.
    pub fn stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.workload.as_mut()?.stdin.take()
    }
    pub fn stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.workload.as_mut()?.stdout.take()
    }
    pub fn stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.workload.as_mut()?.stderr.take()
    }
    pub fn begin_cleanup(&mut self, deadline_ns: u64) -> io::Result<()> {
        self.owner.borrow_mut().admission_closed = true;
        if let Some(child) = &mut self.workload {
            child.stdin.take();
        }
        let clock = now();
        let at = clock.as_ref().ok().copied();
        let result = clock.and_then(|at| self.settlement.begin(deadline_ns, at, &mut self.channel));
        let result = self.checked(result);
        if result.is_err() {
            // Scope's common failed-observation bound still applies when a
            // poisoned/sending-failed lease cannot form a clean wire proof.
            // This is local failure retention only, never a clean bound ACK.
            self.settlement.cap_failed_observation(at, deadline_ns);
        }
        result
    }
    pub fn poll_cleanup(&mut self) -> io::Result<Option<GroupProof>> {
        // A sibling owner fault cannot be rehabilitated by this lease's trace.
        let owner_fault = self.owner.borrow().first_fault;
        if let Some(reason) = owner_fault {
            self.fail(reason.0);
        }
        // Always collect available waits, including on clock failure/expiry.
        let result = self.settlement.poll(
            &mut now,
            self.keeper_pid,
            &mut self.channel,
            &mut self.keeper,
            self.workload.as_mut(),
        );
        // Preserve first specific literal; never replace it with a wrapper.
        if result.is_err() {
            let reason = self
                .settlement
                .progress
                .first_fault
                .unwrap_or_else(|| safe_reason(result.as_ref().err().unwrap()));
            self.fail(reason);
        }
        result
    }
    pub fn report(&self) -> GroupReport {
        GroupReport {
            keeper_pid: self.keeper.id(),
            workload_pid: self.workload.as_ref().map(Child::id),
            first_fault: self.settlement.progress.first_fault,
            secondary_faults: self.settlement.progress.secondary_faults.clone(),
            workload_spawned: self.workload.is_some(),
            workload_status: self.settlement.workload_status,
            keeper_status: self.settlement.keeper_status,
            group_absent: self.settlement.group_absent,
            release_sent: self.settlement.release_sent,
            terminal_eof: self.channel.eof && self.channel.pending.is_empty(),
        }
    }
}
impl Drop for GroupLease {
    fn drop(&mut self) {
        if self.settlement.progress.phase == Phase::Released {
            return;
        }
        self.fail("GROUP_DROPPED_WITHOUT_PROOF");
        // Failure fallback only. One nonblocking collection runs even if the
        // proof deadline passed. Explicit owner must retain report/unknowns.
        self.settlement
            .collect(&mut self.keeper, self.workload.as_mut());
        let deadline = self
            .settlement
            .policy
            .observation_end(now().unwrap_or(0), self.settlement.progress.deadline_ns);
        while now().unwrap_or(deadline) < deadline {
            self.settlement
                .collect(&mut self.keeper, self.workload.as_mut());
            if self.settlement.keeper_status.is_some()
                && (self.workload.is_none() || self.settlement.workload_status.is_some())
            {
                break;
            }
            pause();
        }
    }
}

trait GroupOps {
    fn park(&mut self) -> io::Result<()>;
    fn absent(&mut self) -> io::Result<bool>;
    fn signal(&mut self, signal: libc::c_int) -> io::Result<()>;
    fn abandon_self_group(&mut self) -> io::Result<()>;
    fn disable(&mut self);
}
struct KeeperGroup {
    pid: libc::pid_t,
    park: libc::pid_t,
    parked: bool,
    no_more_group_ops: bool,
}
impl GroupOps for KeeperGroup {
    fn park(&mut self) -> io::Result<()> {
        if !self.parked {
            if self.no_more_group_ops || unsafe { libc::setpgid(0, self.park) } != 0 {
                return Err(refused("KEEPER_PARK"));
            }
            self.parked = true;
        }
        Ok(())
    }
    fn absent(&mut self) -> io::Result<bool> {
        if !self.parked || self.no_more_group_ops {
            return Err(refused("KEEPER_GROUP_AUTHORITY"));
        }
        if unsafe { libc::kill(-self.pid, 0) } == 0 {
            return Ok(false);
        }
        if io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Ok(true);
        }
        Err(refused("KEEPER_GROUP_QUERY"))
    }
    fn signal(&mut self, signal: libc::c_int) -> io::Result<()> {
        if !self.parked || self.no_more_group_ops {
            return Err(refused("KEEPER_GROUP_AUTHORITY"));
        }
        if unsafe { libc::kill(-self.pid, signal) } != 0
            && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        {
            return Err(refused("KEEPER_GROUP_SIGNAL"));
        }
        Ok(())
    }
    fn abandon_self_group(&mut self) -> io::Result<()> {
        // Failure only: parent parking group vanished before PARK. Our live PID
        // still reserves THIS group, of which we are still a member. SIGKILL
        // includes this keeper, so no GONE/proof or subsequent operation exists.
        if self.parked
            || self.no_more_group_ops
            || unsafe { libc::getpid() } != self.pid
            || unsafe { libc::getpgrp() } != self.pid
        {
            return Err(refused("KEEPER_SELF_GROUP_AUTHORITY"));
        }
        self.no_more_group_ops = true;
        if unsafe { libc::kill(-self.pid, libc::SIGKILL) } != 0 {
            return Err(refused("KEEPER_SELF_GROUP_SIGNAL"));
        }
        Err(refused("KEEPER_SELF_GROUP_UNPROVEN"))
    }
    fn disable(&mut self) {
        self.no_more_group_ops = true;
    }
}

// Incremental cleanup: control reductions/EOF are consumed between steps; TERM
// grace is set once, never restarted. Every group syscall checks the same end.
struct KeeperSession<G> {
    group: G,
    lifetime: Lifetime,
    end: u64,
    cleaning: bool,
    parked: bool,
    park_requested: bool,
    term_end: Option<u64>,
    killed: bool,
    gone: bool,
    released: bool,
    abandoned: bool,
    authority_stopped: bool,
    first_fault: Option<&'static str>,
}
impl<G: GroupOps> KeeperSession<G> {
    fn new(group: G, lifetime: Lifetime) -> Self {
        Self {
            group,
            lifetime,
            end: lifetime.hard_end,
            cleaning: false,
            parked: false,
            park_requested: false,
            term_end: None,
            killed: false,
            gone: false,
            released: false,
            abandoned: false,
            authority_stopped: false,
            first_fault: None,
        }
    }
    fn fail(&mut self, reason: &'static str, at: u64) {
        self.first_fault.get_or_insert(reason);
        if !self.cleaning && !self.gone {
            self.cleaning = true;
            self.end = self
                .end
                .min(at.saturating_add(self.lifetime.policy.reserve_ns()));
        }
    }
    fn check_authority(&self) -> io::Result<()> {
        if self.authority_stopped {
            return Err(refused(
                self.first_fault.unwrap_or("KEEPER_AUTHORITY_STOPPED"),
            ));
        }
        Ok(())
    }
    fn stop_authority(&mut self, reason: &'static str, at: u64) {
        self.fail(reason, at);
        if !self.authority_stopped {
            self.authority_stopped = true;
            self.park_requested = false;
            // Logical disable only: do not query/signal a group when a possibly
            // unread stricter bound is unavailable. FAILED/unknown, not GONE.
            self.group.disable();
        }
    }
    fn control(&mut self, body: &str, at: u64) -> io::Result<()> {
        self.check_authority()?;
        if at >= self.end {
            return Err(refused("KEEPER_CLEANUP_DEADLINE"));
        }
        if self.released {
            return Err(refused("KEEPER_TRAILING_RELEASE"));
        }
        if let Some(end) = body.strip_prefix("CLEAN|") {
            self.end = self
                .end
                .min(number(end)?)
                .min(at.saturating_add(self.lifetime.policy.reserve_ns()));
            self.cleaning = true;
            Ok(())
        } else if body == "RELEASE" && self.gone && self.first_fault.is_none() {
            self.released = true;
            Ok(())
        } else {
            Err(refused("KEEPER_STATE"))
        }
    }
    fn check_time(&self, clock: &mut impl FnMut() -> io::Result<u64>) -> io::Result<u64> {
        self.check_authority()?;
        let at = clock()?;
        if at >= self.end {
            return Err(refused("KEEPER_CLEANUP_DEADLINE"));
        }
        Ok(at)
    }
    fn park(&mut self, clock: &mut impl FnMut() -> io::Result<u64>) -> io::Result<()> {
        self.check_time(clock)?;
        if let Err(error) = self.group.park() {
            self.first_fault.get_or_insert("KEEPER_PARK");
            // Check the bound again after the failed syscall. No extra grace.
            self.check_time(clock)?;
            self.abandoned = true;
            self.group.abandon_self_group()?;
            return Err(error);
        }
        self.parked = true;
        Ok(())
    }
    fn step(&mut self, clock: &mut impl FnMut() -> io::Result<u64>) -> io::Result<bool> {
        self.check_authority()?;
        if self.abandoned {
            return Err(refused("KEEPER_SELF_GROUP_UNPROVEN"));
        }
        let at = clock()?;
        if !self.cleaning && at >= self.lifetime.operation_end {
            self.fail("KEEPER_LIFETIME", at);
        }
        if at >= self.end {
            return Err(refused("KEEPER_CLEANUP_DEADLINE"));
        }
        if !self.cleaning || self.gone {
            return Ok(false);
        }
        if !self.parked {
            self.park(clock)?;
        }
        self.check_time(clock)?;
        if self.group.absent()? {
            self.group.disable();
            self.gone = true;
            return Ok(true);
        }
        if let Some(term_end) = self.term_end {
            if !self.killed && clock()? >= term_end {
                self.check_time(clock)?;
                self.killed = true;
                self.group.signal(libc::SIGKILL)?;
            }
        } else {
            let at = self.check_time(clock)?;
            self.term_end = Some(at.saturating_add(self.lifetime.policy.term_ns()));
            self.group.signal(libc::SIGTERM)?;
        }
        Ok(false)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Turn {
    Again,
    Finished,
}
fn keeper_turn<G: GroupOps, S: Read + Write>(
    session: &mut KeeperSession<G>,
    channel: &mut Channel<S>,
    pid: libc::pid_t,
    park: u64,
    sid: u64,
    clock: &mut impl FnMut() -> io::Result<u64>,
) -> io::Result<Turn> {
    session.check_authority()?;
    let at = clock()?;
    if at >= session.end {
        return Err(refused("KEEPER_CLEANUP_DEADLINE"));
    }
    // Drain bounded traffic BEFORE the next syscall step. When the parse
    // cap is reached, yield without group ops; reductions cannot sit behind
    // already-buffered control traffic while signalling proceeds.
    let mut drained = false;
    for _ in 0..8 {
        match channel.receive_event() {
            Ok(Receive::Frame(body))
                if session.first_fault.is_none() || body.starts_with("CLEAN|") =>
            {
                let result = if body == "PARK"
                    && !session.parked
                    && !session.cleaning
                    && !session.park_requested
                {
                    // Pure bounded intent only. Even membership changes and
                    // the failure self-kill must wait for the minimum drain.
                    session.park_requested = true;
                    Ok(())
                } else {
                    session.control(&body, clock()?)
                };
                if result.is_err() {
                    session.fail("KEEPER_CONTROL", clock()?);
                }
            }
            // Failure remains irreversible. Only canonical CLEAN can tighten
            // its bound; no PARK/RELEASE/GONE/proof path is reopened.
            Ok(Receive::Frame(_)) => (),
            Ok(Receive::Rejected(_)) => session.fail("KEEPER_CHANNEL", clock()?),
            Ok(Receive::BudgetYield) => return Ok(Turn::Again),
            Ok(Receive::Quiescent | Receive::Eof) => {
                drained = true;
                break;
            }
            Err(_) => {
                // No known frame boundary/drain fact: stop authority rather
                // than act under an end that may have been superseded unread.
                session.stop_authority("KEEPER_CHANNEL", clock()?);
                return Err(refused(session.first_fault.unwrap()));
            }
        }
    }
    if channel.eof {
        if session.released && session.first_fault.is_none() {
            session.check_time(clock)?;
            return Ok(Turn::Finished);
        }
        session.fail("KEEPER_OWNER_EOF", clock()?);
    }
    if !drained {
        return Ok(Turn::Again);
    }
    if session.park_requested {
        session.park_requested = false;
        if session.first_fault.is_none() {
            // All readable bounds are now reduced. Fresh end checks precede
            // PARK and any own-group failure fallback inside park().
            session.park(clock)?;
            session.check_time(clock)?;
            if channel.send(&format!("PARKED|{pid}|{park}|{sid}")).is_err() {
                // Preserve failure cleanup if the response itself fails after
                // membership changed; never turn a send fault into a clean exit.
                session.fail("KEEPER_CONTROL", clock()?);
            }
        }
    }
    match session.step(clock) {
        Ok(true) if session.first_fault.is_none() => channel.send(&format!("GONE|{pid}"))?,
        Ok(_) => (),
        Err(error) => return Err(error),
    }
    if session.gone {
        if let Some(reason) = session.first_fault {
            return Err(refused(reason));
        }
    }
    Ok(Turn::Again)
}

/// Entry point of the dev-only keeper example. No product code calls this.
pub fn keeper_main(arguments: impl IntoIterator<Item = impl AsRef<OsStr>>) -> io::Result<()> {
    let values: Vec<String> = arguments
        .into_iter()
        .map(|v| {
            v.as_ref()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| refused("KEEPER_ARG"))
        })
        .collect::<io::Result<_>>()?;
    if !matches!(values.len(), 6 | 7) || !nonce_valid(&values[1]) {
        return Err(refused("KEEPER_ARGS"));
    }
    let fd = number(&values[0])?;
    let parent = number(&values[2])?;
    let park = number(&values[3])?;
    let sid = number(&values[4])?;
    let deadline = number(&values[5])?;
    let policy = CleanupPolicy::keeper_tag(values.get(6).map(String::as_str))?;
    let admitted_at = now()?;
    let lifetime = if policy == CleanupPolicy::Legacy {
        Lifetime::new(admitted_at, deadline)?
    } else {
        Lifetime::with_policy(admitted_at, deadline, policy)?
    };
    if !(4..=1024).contains(&fd)
        || parent != unsafe { libc::getppid() } as u64
        || park == 0
        || park > i32::MAX as u64
        || sid != unsafe { libc::getsid(0) } as u64
        || unsafe { libc::getpid() } != unsafe { libc::getpgrp() }
        || unsafe { libc::getpid() } as u64 == sid
    {
        return Err(refused("KEEPER_IDENTITY"));
    }
    let mut kind: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&kind) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd as i32,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut libc::c_int).cast(),
            &mut length,
        )
    } != 0
        || kind != libc::SOCK_STREAM
    {
        return Err(refused("KEEPER_FD"));
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd as i32) };
    let mut channel = Channel::new(stream, values[1].clone())?;
    let group = KeeperGroup {
        pid: unsafe { libc::getpid() },
        park: park as libc::pid_t,
        parked: false,
        no_more_group_ops: false,
    };
    let pid = group.pid;
    let mut session = KeeperSession::new(group, lifetime);
    let mut clock = now;
    channel.send(&format!("READY|{pid}|{pid}|{sid}"))?;
    loop {
        match keeper_turn(&mut session, &mut channel, pid, park, sid, &mut clock)? {
            Turn::Finished => return Ok(()),
            Turn::Again => pause(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_group_operation_after_absence_or_release_or_fault() {
        let mut state = Progress::new(100);
        for (from, to) in [
            (Phase::AwaitReady, Phase::Ready),
            (Phase::Ready, Phase::Joined),
            (Phase::Joined, Phase::Parked),
            (Phase::Parked, Phase::Cleaning),
        ] {
            state.advance(from, to).unwrap();
        }
        assert!(state.may_touch_group());
        state.advance(Phase::Cleaning, Phase::Gone).unwrap();
        assert!(!state.may_touch_group());
        state.advance(Phase::Gone, Phase::Released).unwrap();
        assert!(!state.may_touch_group());
        state.fail("FIRST");
        state.fail("SECOND");
        assert_eq!(state.first_fault, Some("FIRST"));
        assert!(!state.may_touch_group());
    }
    #[test]
    fn admission_cannot_skip_ready_join_or_park_and_deadline_never_extends() {
        let mut state = Progress::new(100);
        assert!(state.advance(Phase::AwaitReady, Phase::Parked).is_err());
        // A stale predecessor must never rehabilitate this machine after failure.
        assert!(state.advance(Phase::Ready, Phase::Joined).is_err());
        assert!(state.advance(Phase::Failed, Phase::Ready).is_err());
        state.shorten(200);
        assert_eq!(state.deadline_ns, 100);
        state.shorten(50);
        assert_eq!(state.deadline_ns, 50);
    }
    #[test]
    fn every_unlisted_transition_is_refused_and_stays_failed() {
        let phases = [
            Phase::AwaitReady,
            Phase::Ready,
            Phase::Joined,
            Phase::Parked,
            Phase::Cleaning,
            Phase::Gone,
            Phase::Released,
            Phase::Failed,
        ];
        for from in phases {
            for to in phases {
                let allowed = matches!(
                    (from, to),
                    (Phase::AwaitReady, Phase::Ready)
                        | (Phase::Ready, Phase::Joined)
                        | (Phase::Joined, Phase::Parked)
                        | (Phase::Parked, Phase::Cleaning)
                        | (Phase::Cleaning, Phase::Gone)
                        | (Phase::Gone, Phase::Released)
                );
                let mut state = Progress::new(100);
                state.phase = from;
                assert_eq!(state.advance(from, to).is_ok(), allowed, "{from:?}->{to:?}");
                if !allowed {
                    assert_eq!(state.phase, Phase::Failed);
                    assert!(!state.may_touch_group());
                }
            }
        }
    }
    #[test]
    fn content_free_channel_checks_nonce_and_bounds_partial_input() {
        let (left, mut right) = UnixStream::pair().unwrap();
        right
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut channel = Channel::new(left, "a".repeat(32)).unwrap();
        right
            .write_all(format!("{}|PARK\n", "b".repeat(32)).as_bytes())
            .unwrap();
        assert_eq!(channel.receive().unwrap_err().to_string(), "GROUP_NONCE");
        let (left, mut right) = UnixStream::pair().unwrap();
        right
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut channel = Channel::new(left, "a".repeat(32)).unwrap();
        right.write_all(&[b'x'; MAX_FRAME]).unwrap();
        assert_eq!(
            channel.receive().unwrap_err().to_string(),
            "GROUP_FRAME_LIMIT"
        );
    }
    #[test]
    fn content_free_channel_preserves_split_frames_and_eof() {
        let (left, mut right) = UnixStream::pair().unwrap();
        right
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut channel = Channel::new(left, "a".repeat(32)).unwrap();
        right.write_all("a".repeat(32).as_bytes()).unwrap();
        assert!(channel.receive().unwrap().is_none());
        right.write_all(b"|PARK\n").unwrap();
        assert_eq!(channel.receive().unwrap().as_deref(), Some("PARK"));
        right.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(channel.receive().unwrap().is_none());
        assert!(channel.eof);
    }

    // R19 regression drivers execute the actual settlement/session methods with
    // finite fake wait/syscall results and Unix socketpairs, never child actors.
    use std::{collections::VecDeque, os::unix::process::ExitStatusExt};
    #[derive(Default)]
    struct FakeWait {
        steps: VecDeque<io::Result<Option<ExitStatus>>>,
        calls: usize,
    }
    impl DirectWait for FakeWait {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.calls += 1;
            self.steps.pop_front().unwrap_or(Ok(None))
        }
    }
    fn exited() -> io::Result<Option<ExitStatus>> {
        Ok(Some(ExitStatus::from_raw(0)))
    }
    fn parent_fixture() -> (Settlement, Channel, UnixStream) {
        let (left, right) = UnixStream::pair().unwrap();
        right
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        right
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut state = Settlement::new(1000);
        state.cleanup_sent = true;
        state.progress.phase = Phase::Cleaning;
        (state, Channel::new(left, "a".repeat(32)).unwrap(), right)
    }
    fn frame(peer: &mut UnixStream, body: &str) {
        peer.write_all(format!("{}|{body}\n", "a".repeat(32)).as_bytes())
            .unwrap();
    }
    #[test]
    fn consumed_command_type_cannot_return_a_stale_join_callback() {
        let _: fn(&mut GroupLease, Command) -> io::Result<()> = GroupLease::start;
    }
    #[test]
    fn duplicate_gone_between_pending_and_completed_wait_is_sticky() {
        let (mut state, mut channel, mut peer) = parent_fixture();
        let mut keeper = FakeWait::default();
        let mut workload = FakeWait {
            steps: [Ok(None), exited()].into(),
            calls: 0,
        };
        frame(&mut peer, "GONE|42");
        assert!(state
            .poll(
                &mut || Ok(1),
                42,
                &mut channel,
                &mut keeper,
                Some(&mut workload)
            )
            .unwrap()
            .is_none());
        assert_eq!(state.progress.phase, Phase::Gone);
        assert!(!state.release_sent);
        frame(&mut peer, "GONE|42");
        assert_eq!(
            state
                .poll(
                    &mut || Ok(2),
                    42,
                    &mut channel,
                    &mut keeper,
                    Some(&mut workload)
                )
                .unwrap_err()
                .to_string(),
            "GROUP_TERMINAL"
        );
        assert_eq!(state.progress.first_fault, Some("GROUP_TERMINAL"));
        assert!(state.workload_status.is_some());
        assert!(!state.release_sent);
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        keeper.steps.push_back(exited());
        assert!(state
            .poll(
                &mut || Ok(3),
                42,
                &mut channel,
                &mut keeper,
                Some(&mut workload)
            )
            .is_err());
        assert!(state.keeper_status.is_some());
        assert_eq!(state.progress.first_fault, Some("GROUP_TERMINAL"));
    }
    #[test]
    fn bad_nonce_cannot_be_rehabilitated_by_gone_and_available_waits() {
        let (mut state, mut channel, mut peer) = parent_fixture();
        let mut keeper = FakeWait::default();
        let mut workload = FakeWait::default();
        peer.write_all(format!("{}|GONE|42\n", "b".repeat(32)).as_bytes())
            .unwrap();
        assert_eq!(
            state
                .poll(
                    &mut || Ok(1),
                    42,
                    &mut channel,
                    &mut keeper,
                    Some(&mut workload)
                )
                .unwrap_err()
                .to_string(),
            "GROUP_NONCE"
        );
        frame(&mut peer, "GONE|42");
        keeper.steps.push_back(exited());
        workload.steps.push_back(exited());
        assert_eq!(
            state
                .poll(
                    &mut || Ok(2),
                    42,
                    &mut channel,
                    &mut keeper,
                    Some(&mut workload)
                )
                .unwrap_err()
                .to_string(),
            "GROUP_NONCE"
        );
        assert!(state.keeper_status.is_some() && state.workload_status.is_some());
        assert!(!state.group_absent && !state.release_sent);
    }
    #[test]
    fn park_failure_and_expiry_still_collect_both_direct_waits_and_first_reason() {
        let (mut state, mut channel, _peer) = parent_fixture();
        state.progress.fail("GROUP_PARK");
        let mut keeper = FakeWait {
            steps: [exited()].into(),
            calls: 0,
        };
        let mut workload = FakeWait {
            steps: [exited()].into(),
            calls: 0,
        };
        assert_eq!(
            state
                .poll(
                    &mut || Ok(1000),
                    42,
                    &mut channel,
                    &mut keeper,
                    Some(&mut workload)
                )
                .unwrap_err()
                .to_string(),
            "GROUP_PARK"
        );
        assert_eq!((keeper.calls, workload.calls), (1, 1));
        assert!(state.keeper_status.is_some() && state.workload_status.is_some());
        assert_eq!(state.progress.secondary_faults, ["GROUP_CLEANUP_DEADLINE"]);
    }
    #[test]
    fn wait_error_does_not_skip_other_wait_or_erase_original_fault() {
        let (mut state, mut channel, _peer) = parent_fixture();
        state.progress.fail("GROUP_PARK");
        let mut keeper = FakeWait {
            steps: [exited()].into(),
            calls: 0,
        };
        let mut workload = FakeWait {
            steps: [Err(refused("private OS text")), exited()].into(),
            calls: 0,
        };
        assert_eq!(
            state
                .poll(
                    &mut || Ok(1),
                    42,
                    &mut channel,
                    &mut keeper,
                    Some(&mut workload)
                )
                .unwrap_err()
                .to_string(),
            "GROUP_PARK"
        );
        assert!(state.keeper_status.is_some() && state.workload_status.is_none());
        assert_eq!(state.progress.secondary_faults, ["GROUP_WORKLOAD_WAIT"]);
        assert!(state
            .poll(
                &mut || Ok(2),
                42,
                &mut channel,
                &mut keeper,
                Some(&mut workload)
            )
            .is_err());
        assert!(state.workload_status.is_some());
    }
    #[test]
    fn expired_unfaulted_poll_collects_available_waits_but_never_proves_clean() {
        let (mut state, mut channel, _peer) = parent_fixture();
        let mut keeper = FakeWait {
            steps: [exited()].into(),
            calls: 0,
        };
        let mut workload = FakeWait {
            steps: [exited()].into(),
            calls: 0,
        };
        assert_eq!(
            state
                .poll(
                    &mut || Ok(1000),
                    42,
                    &mut channel,
                    &mut keeper,
                    Some(&mut workload)
                )
                .unwrap_err()
                .to_string(),
            "GROUP_CLEANUP_DEADLINE"
        );
        assert!(state.keeper_status.is_some() && state.workload_status.is_some());
        assert!(!state.group_absent);
    }
    #[test]
    fn single_gone_waits_and_empty_eof_positive_control() {
        let (mut state, mut channel, mut peer) = parent_fixture();
        let mut keeper = FakeWait {
            steps: [Ok(None), exited()].into(),
            calls: 0,
        };
        let mut workload = FakeWait {
            steps: [exited()].into(),
            calls: 0,
        };
        frame(&mut peer, "GONE|42");
        assert!(state
            .poll(
                &mut || Ok(1),
                42,
                &mut channel,
                &mut keeper,
                Some(&mut workload)
            )
            .unwrap()
            .is_none());
        let mut bytes = [0; 128];
        let n = peer.read(&mut bytes).unwrap();
        assert_eq!(
            &bytes[..n],
            format!("{}|RELEASE\n", "a".repeat(32)).as_bytes()
        );
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        let proof = state
            .poll(
                &mut || Ok(2),
                42,
                &mut channel,
                &mut keeper,
                Some(&mut workload),
            )
            .unwrap()
            .unwrap();
        assert!(proof.group_absent && proof.keeper_status.success());
        assert_eq!(state.progress.phase, Phase::Released);
    }
    #[test]
    fn gone_without_clean_and_early_eof_are_refused() {
        for clean in [false, true] {
            let (mut state, mut channel, mut peer) = parent_fixture();
            state.cleanup_sent = clean;
            if !clean {
                frame(&mut peer, "GONE|42");
            } else {
                peer.shutdown(std::net::Shutdown::Write).unwrap();
            }
            assert!(state
                .poll(
                    &mut || Ok(1),
                    42,
                    &mut channel,
                    &mut FakeWait::default(),
                    None::<&mut FakeWait>
                )
                .is_err());
            assert_eq!(
                state.progress.first_fault,
                Some(if clean {
                    "GROUP_EARLY_EOF"
                } else {
                    "GROUP_CLEANUP_NOT_STARTED"
                })
            );
        }
    }
    #[test]
    fn missing_eof_and_partial_trailing_bytes_never_prove_clean() {
        for partial in [false, true] {
            let (mut state, mut channel, mut peer) = parent_fixture();
            let mut keeper = FakeWait {
                steps: [Ok(None), exited()].into(),
                calls: 0,
            };
            frame(&mut peer, "GONE|42");
            assert!(state
                .poll(
                    &mut || Ok(1),
                    42,
                    &mut channel,
                    &mut keeper,
                    None::<&mut FakeWait>
                )
                .unwrap()
                .is_none());
            if partial {
                peer.write_all(b"partial").unwrap();
                peer.shutdown(std::net::Shutdown::Write).unwrap();
            }
            let result = state.poll(
                &mut || Ok(2),
                42,
                &mut channel,
                &mut keeper,
                None::<&mut FakeWait>,
            );
            if partial {
                assert_eq!(result.unwrap_err().to_string(), "GROUP_TRUNCATED");
            } else {
                assert!(result.unwrap().is_none());
            }
        }
    }
    #[derive(Default)]
    struct FakeGroup {
        events: Vec<&'static str>,
        absent: bool,
        fail_park: bool,
        disabled: bool,
    }
    impl GroupOps for FakeGroup {
        fn park(&mut self) -> io::Result<()> {
            self.events.push("park");
            if self.fail_park {
                Err(refused("KEEPER_PARK"))
            } else {
                Ok(())
            }
        }
        fn absent(&mut self) -> io::Result<bool> {
            assert!(!self.disabled);
            self.events.push("query");
            Ok(self.absent)
        }
        fn signal(&mut self, sig: i32) -> io::Result<()> {
            assert!(!self.disabled);
            self.events
                .push(if sig == libc::SIGTERM { "term" } else { "kill" });
            Ok(())
        }
        fn abandon_self_group(&mut self) -> io::Result<()> {
            assert!(!self.disabled);
            self.events.push("self-kill");
            self.disabled = true;
            Err(refused("KEEPER_SELF_GROUP_UNPROVEN"))
        }
        fn disable(&mut self) {
            self.events.push("disable");
            self.disabled = true;
        }
    }
    fn helper_fixture() -> KeeperSession<FakeGroup> {
        KeeperSession::new(
            FakeGroup::default(),
            Lifetime::new(0, 10 * CLEANUP_NS).unwrap(),
        )
    }
    #[test]
    fn lifetime_reserves_cleanup_and_equality_never_signals_after_hard_end() {
        assert!(Lifetime::new(0, CLEANUP_NS).is_err());
        let mut state = helper_fixture();
        let trigger = state.lifetime.operation_end;
        assert!(!state.step(&mut || Ok(trigger - 1)).unwrap());
        assert!(state.group.events.is_empty());
        assert!(!state.step(&mut || Ok(trigger)).unwrap());
        assert_eq!(state.group.events, ["park", "query", "term"]);
        assert_eq!(state.first_fault, Some("KEEPER_LIFETIME"));
        let saved = state.group.events.clone();
        assert!(state.step(&mut || Ok(10 * CLEANUP_NS)).is_err());
        assert_eq!(state.group.events, saved);
    }
    #[test]
    fn pre_park_parent_loss_uses_failure_only_self_group_fallback_once() {
        let mut state = helper_fixture();
        state.group.fail_park = true;
        state.fail("KEEPER_OWNER_EOF", 1);
        assert!(state.step(&mut || Ok(1)).is_err());
        assert_eq!(state.group.events, ["park", "self-kill"]);
        assert!(!state.gone);
        assert!(state.step(&mut || Ok(2)).is_err());
        assert_eq!(state.group.events, ["park", "self-kill"]);
        assert_eq!(state.first_fault, Some("KEEPER_OWNER_EOF"));
    }
    #[test]
    fn decreasing_bounds_during_term_do_not_restart_grace_or_extend_end() {
        let mut state = helper_fixture();
        state.control("CLEAN|100000000", 1).unwrap();
        assert!(!state.step(&mut || Ok(2)).unwrap());
        let grace = state.term_end.unwrap();
        state.control("CLEAN|200000000", 3).unwrap();
        assert_eq!(state.end, 100000000);
        state.control("CLEAN|100000000", 4).unwrap();
        state.control("CLEAN|1000", 5).unwrap();
        assert_eq!(state.end, 1000);
        assert_eq!(state.term_end, Some(grace));
        let saved = state.group.events.clone();
        assert!(state.step(&mut || Ok(1000)).is_err());
        assert_eq!(state.group.events, saved);
    }
    #[test]
    fn shortening_after_gone_bounds_release_and_never_touches_group() {
        let mut state = helper_fixture();
        state.group.absent = true;
        state.control("CLEAN|1000", 1).unwrap();
        assert!(state.step(&mut || Ok(2)).unwrap());
        let saved = state.group.events.clone();
        state.control("CLEAN|100", 3).unwrap();
        assert!(!state.step(&mut || Ok(4)).unwrap());
        assert_eq!(state.group.events, saved);
        assert!(state.control("RELEASE", 100).is_err());
        assert!(!state.released);
    }
    #[test]
    fn syscall_time_crossing_end_stops_before_next_group_operation() {
        let mut state = helper_fixture();
        state.control("CLEAN|100", 1).unwrap();
        let mut times = VecDeque::from([Ok(2), Ok(2), Ok(100)]);
        assert!(state.step(&mut || times.pop_front().unwrap()).is_err());
        assert_eq!(state.group.events, ["park"]);
    }
    #[test]
    fn pre_ready_cancellation_requires_the_actual_ready_before_gone() {
        for invalid in [false, true] {
            let (mut state, mut channel, mut peer) = parent_fixture();
            state.progress.phase = Phase::AwaitReady;
            state.expected_ready = Some("READY|42|42|7".to_owned());
            state.progress.begin_cleanup().unwrap();
            let mut keeper = FakeWait::default();
            if invalid {
                frame(&mut peer, "GONE|42");
                assert_eq!(
                    state
                        .poll(
                            &mut || Ok(1),
                            42,
                            &mut channel,
                            &mut keeper,
                            None::<&mut FakeWait>
                        )
                        .unwrap_err()
                        .to_string(),
                    "GROUP_READY"
                );
                assert!(!state.group_absent);
            } else {
                frame(&mut peer, "READY|42|42|7");
                frame(&mut peer, "GONE|42");
                assert!(state
                    .poll(
                        &mut || Ok(1),
                        42,
                        &mut channel,
                        &mut keeper,
                        None::<&mut FakeWait>
                    )
                    .unwrap()
                    .is_none());
                assert!(state.expected_ready.is_none());
                assert!(!state.group_absent);
                assert!(state
                    .poll(
                        &mut || Ok(2),
                        42,
                        &mut channel,
                        &mut keeper,
                        None::<&mut FakeWait>
                    )
                    .unwrap()
                    .is_none());
                assert!(state.group_absent && state.release_sent);
            }
        }
    }
    #[test]
    fn parent_driver_sends_each_decrease_and_explicitly_refuses_after_seal() {
        let (mut state, mut channel, mut peer) = parent_fixture();
        state.cleanup_sent = false;
        state.progress.phase = Phase::Parked;
        state.begin(900, 1, &mut channel).unwrap();
        state.begin(800, 2, &mut channel).unwrap();
        state.begin(950, 3, &mut channel).unwrap();
        assert_eq!(state.progress.deadline_ns, 800);
        let mut bytes = [0; 256];
        let n = peer.read(&mut bytes).unwrap();
        assert_eq!(
            &bytes[..n],
            format!("{0}|CLEAN|900\n{0}|CLEAN|800\n", "a".repeat(32)).as_bytes()
        );
        state.release_sent = true;
        assert_eq!(
            state.begin(700, 4, &mut channel).unwrap_err().to_string(),
            "GROUP_BOUND_SEALED"
        );
        assert_eq!(state.progress.deadline_ns, 800);
        state.begin(800, 5, &mut channel).unwrap();
    }
    #[test]
    fn peer_death_collects_direct_keeper_wait_and_cannot_look_clean() {
        let (mut state, mut channel, peer) = parent_fixture();
        drop(peer);
        let mut keeper = FakeWait {
            steps: [exited()].into(),
            calls: 0,
        };
        let mut workload = FakeWait::default();
        assert_eq!(
            state
                .poll(
                    &mut || Ok(1),
                    42,
                    &mut channel,
                    &mut keeper,
                    Some(&mut workload)
                )
                .unwrap_err()
                .to_string(),
            "GROUP_EARLY_EOF"
        );
        assert!(state.keeper_status.is_some());
        assert!(state.workload_status.is_none());
        workload.steps.push_back(exited());
        assert!(state
            .poll(
                &mut || Ok(1000),
                42,
                &mut channel,
                &mut keeper,
                Some(&mut workload)
            )
            .is_err());
        assert!(state.workload_status.is_some());
        assert_eq!(state.progress.first_fault, Some("GROUP_EARLY_EOF"));
    }
    #[test]
    fn proof_and_release_recheck_clock_after_receiving_bytes() {
        for final_proof in [false, true] {
            let (mut state, mut channel, mut peer) = parent_fixture();
            let mut keeper = FakeWait::default();
            let mut times = if final_proof {
                state.progress.phase = Phase::Gone;
                state.group_absent = true;
                state.release_sent = true;
                keeper.steps.push_back(exited());
                peer.shutdown(std::net::Shutdown::Write).unwrap();
                VecDeque::from([Ok(1), Ok(1), Ok(1000)])
            } else {
                frame(&mut peer, "GONE|42");
                VecDeque::from([Ok(1), Ok(1000)])
            };
            assert_eq!(
                state
                    .poll(
                        &mut || times.pop_front().unwrap(),
                        42,
                        &mut channel,
                        &mut keeper,
                        None::<&mut FakeWait>
                    )
                    .unwrap_err()
                    .to_string(),
                "GROUP_CLEANUP_DEADLINE"
            );
            assert_eq!(state.progress.first_fault, Some("GROUP_CLEANUP_DEADLINE"));
            assert_ne!(state.progress.phase, Phase::Released);
            if !final_proof {
                assert!(!state.release_sent);
            }
        }
    }
    // R20 drives the EXACT outer keeper turn called by keeper_main. The
    // extracted baseline deliberately retains both reviewed defects for RED.
    fn r20_failed_reduction(expiry: bool) {
        let mut session = helper_fixture();
        let at = if expiry {
            session.lifetime.operation_end
        } else {
            2
        };
        if !expiry {
            session.control("CLEAN|1000000000", 1).unwrap();
        }
        session.step(&mut || Ok(at)).unwrap();
        if !expiry {
            session.fail("KEEPER_CHANNEL", at);
        }
        let first = session.first_fault;
        assert!(first.is_some());
        let events = session.group.events.clone();
        let grace = session.term_end;
        let (left, right) = UnixStream::pair().unwrap();
        let mut parent_channel = Channel::new(left, "a".repeat(32)).unwrap();
        let mut keeper_channel = Channel::new(right, "a".repeat(32)).unwrap();
        let mut parent = Settlement::new(session.lifetime.hard_end);
        parent.progress.phase = Phase::Parked;
        let end = at + 5_000_000;
        parent
            .begin(end, at + 1_000_000, &mut parent_channel)
            .unwrap();
        assert_eq!(parent.progress.deadline_ns, end);
        let result = keeper_turn(&mut session, &mut keeper_channel, 42, 7, 7, &mut || {
            Ok(at + 6_000_000)
        });
        assert_eq!(
            session.end, end,
            "helper discarded the bound accepted by the actual parent driver"
        );
        assert_eq!(session.first_fault, first);
        assert_eq!(session.term_end, grace);
        assert_eq!(
            session.group.events, events,
            "no group operation after accepted end"
        );
        assert_eq!(result.unwrap_err().to_string(), "KEEPER_CLEANUP_DEADLINE");
        assert!(!session.gone && !session.released);
    }
    #[test]
    fn r20_expiry_fault_still_consumes_parent_accepted_reduction() {
        r20_failed_reduction(true);
    }
    #[test]
    fn r20_prior_clean_then_fault_still_consumes_parent_accepted_reduction() {
        r20_failed_reduction(false);
    }

    enum ReadAction {
        Bytes(Vec<u8>),
        Interrupted,
        Fault,
    }
    #[derive(Default)]
    struct ScriptedIo {
        actions: VecDeque<ReadAction>,
        reads: usize,
        output: Vec<u8>,
    }
    impl Read for ScriptedIo {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            match self.actions.pop_front() {
                Some(ReadAction::Bytes(bytes)) => {
                    assert!(bytes.len() <= buffer.len());
                    buffer[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                Some(ReadAction::Interrupted) => Err(io::Error::from(io::ErrorKind::Interrupted)),
                Some(ReadAction::Fault) => Err(io::Error::other("scripted read fault")),
                None => Err(io::Error::from(io::ErrorKind::WouldBlock)),
            }
        }
    }
    impl Write for ScriptedIo {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn r20_budget_yield(interrupted: bool) {
        let mut session = helper_fixture();
        session.control("CLEAN|1000000000", 1).unwrap();
        session.step(&mut || Ok(2)).unwrap();
        let events = session.group.events.clone();
        let bytes = format!("{}|CLEAN|100\n", "a".repeat(32)).into_bytes();
        let actions = if interrupted {
            (0..7)
                .map(|_| ReadAction::Interrupted)
                .chain([ReadAction::Bytes(bytes)])
                .collect()
        } else {
            bytes[..7]
                .iter()
                .map(|b| ReadAction::Bytes(vec![*b]))
                .chain([ReadAction::Bytes(bytes[7..].to_vec())])
                .collect()
        };
        let mut channel = Channel {
            stream: ScriptedIo {
                actions,
                ..Default::default()
            },
            nonce: "a".repeat(32),
            pending: vec![],
            wire: 0,
            eof: false,
        };
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(150)).unwrap(),
            Turn::Again
        );
        assert_eq!(channel.stream.reads, 8, "no ninth read in a receive pass");
        assert!(channel.pending.ends_with(b"\n"));
        assert_eq!(
            session.group.events, events,
            "budget yield must not authorize group operations"
        );
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(150))
                .unwrap_err()
                .to_string(),
            "KEEPER_CLEANUP_DEADLINE"
        );
        assert_eq!(session.end, 100);
        assert_eq!(session.group.events, events);
        assert!(channel.stream.output.is_empty());
    }
    #[test]
    fn r20_eighth_short_read_completion_yields_before_group_operations() {
        r20_budget_yield(false);
    }
    #[test]
    fn r20_seven_interruptions_then_completion_yields_before_group_operations() {
        r20_budget_yield(true);
    }
    fn scripted_channel(bytes: &[u8]) -> Channel<ScriptedIo> {
        Channel {
            stream: ScriptedIo {
                actions: bytes
                    .chunks(MAX_FRAME)
                    .map(|b| ReadAction::Bytes(b.to_vec()))
                    .collect(),
                ..Default::default()
            },
            nonce: "a".repeat(32),
            pending: vec![],
            wire: 0,
            eof: false,
        }
    }
    #[test]
    fn r20_failed_control_only_tightens_and_never_reopens_normal_trace() {
        let mut session = helper_fixture();
        let at = session.lifetime.operation_end;
        session.step(&mut || Ok(at)).unwrap();
        let first = session.first_fault;
        let grace = session.term_end;
        let end = at + 5_000_000;
        let payload=format!("{0}|PARK\n{0}|RELEASE\n{0}|GONE|42\n{0}|CLEAN|{1}\n{0}|CLEAN|0001\n{0}|CLEAN|999999999999\n", "a".repeat(32),end);
        let mut channel = scripted_channel(payload.as_bytes());
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                at + 1_000_000
            ))
            .unwrap(),
            Turn::Again
        );
        assert_eq!(session.end, end);
        assert_eq!(session.first_fault, first);
        assert_eq!(session.term_end, grace);
        assert!(!session.released && !session.gone);
        assert!(channel.stream.output.is_empty());
    }
    #[test]
    fn r20_control_frame_cap_yields_until_last_buffered_minimum_is_consumed() {
        let mut session = helper_fixture();
        session.control("CLEAN|1000000000", 1).unwrap();
        session.step(&mut || Ok(2)).unwrap();
        let events = session.group.events.clone();
        let payload = (1..=9)
            .rev()
            .map(|n| format!("{}|CLEAN|{}\n", "a".repeat(32), n * 100))
            .collect::<String>();
        let mut channel = scripted_channel(payload.as_bytes());
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(150)).unwrap(),
            Turn::Again
        );
        assert_eq!(session.end, 200);
        assert_eq!(session.group.events, events);
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(150))
                .unwrap_err()
                .to_string(),
            "KEEPER_CLEANUP_DEADLINE"
        );
        assert_eq!(session.end, 100);
        assert_eq!(session.group.events, events);
    }
    #[test]
    fn r20_interrupted_empty_budget_yields_before_proven_quiescence() {
        let mut session = helper_fixture();
        session.control("CLEAN|1000", 1).unwrap();
        session.step(&mut || Ok(2)).unwrap();
        let events = session.group.events.clone();
        let mut channel = scripted_channel(&[]);
        channel.stream.actions = (0..8).map(|_| ReadAction::Interrupted).collect();
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(3)).unwrap(),
            Turn::Again
        );
        assert_eq!(channel.stream.reads, 8);
        assert_eq!(session.group.events, events);
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(4)).unwrap(),
            Turn::Again
        );
        assert_eq!(channel.stream.reads, 9);
        let mut expected = events;
        expected.push("query");
        assert_eq!(session.group.events, expected);
    }
    #[test]
    fn r20_actual_turn_normal_park_gone_release_eof_positive_control() {
        let mut session = helper_fixture();
        session.group.absent = true;
        let payload = format!("{0}|PARK\n{0}|CLEAN|1000\n", "a".repeat(32));
        let mut channel = scripted_channel(payload.as_bytes());
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(1)).unwrap(),
            Turn::Again
        );
        assert!(session.gone);
        assert!(session.first_fault.is_none());
        assert_eq!(
            channel.stream.output,
            format!("{0}|PARKED|42|7|7\n{0}|GONE|42\n", "a".repeat(32)).as_bytes()
        );
        let events = session.group.events.clone();
        channel.stream.actions.push_back(ReadAction::Bytes(
            format!("{}|RELEASE\n", "a".repeat(32)).into_bytes(),
        ));
        channel.stream.actions.push_back(ReadAction::Bytes(vec![]));
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(2)).unwrap(),
            Turn::Finished
        );
        assert!(session.released && channel.eof);
        assert_eq!(session.group.events, events);
    }
    // R21 tests use the same keeper_turn and real parent reducer, not a second
    // parser/state model. All group operations below are fake event recording.
    fn r21_parent_frame(end: u64, at: u64) -> Vec<u8> {
        let (left, mut right) = UnixStream::pair().unwrap();
        right
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        right
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut channel = Channel::new(left, "a".repeat(32)).unwrap();
        let mut parent = Settlement::new(20 * CLEANUP_NS);
        parent.progress.phase = Phase::Parked;
        parent.begin(end, at, &mut channel).unwrap();
        assert_eq!(parent.progress.deadline_ns, end);
        let expected = format!("{}|CLEAN|{end}\n", "a".repeat(32)).into_bytes();
        let mut frame = vec![0; expected.len()];
        right.read_exact(&mut frame).unwrap();
        assert_eq!(frame, expected);
        frame
    }
    fn r21_rejected_prefix(prior_fault: bool, invalid_number: bool) {
        let mut session = helper_fixture();
        let base = if prior_fault {
            session.lifetime.operation_end
        } else {
            2
        };
        if !prior_fault {
            session.control("CLEAN|1000000000", 1).unwrap();
        }
        session.step(&mut || Ok(base)).unwrap();
        let original = session.first_fault;
        let grace = session.term_end;
        let events = session.group.events.clone();
        let end = base + 100_000_000;
        let mut bytes = if invalid_number {
            format!("{}|CLEAN|0001\n", "a".repeat(32)).into_bytes()
        } else {
            format!("{}|PARK\n", "b".repeat(32)).into_bytes()
        };
        bytes.extend(r21_parent_frame(end, base + 10_000_000));
        assert!(bytes.len() < MAX_FRAME);
        let mut channel = scripted_channel(&bytes);
        let result = keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || {
            Ok(base + 150_000_000)
        });
        assert_eq!(
            session.group.events, events,
            "rejected line must not authorize a step before buffered CLEAN"
        );
        assert_eq!(session.end, end);
        assert_eq!(session.term_end, grace);
        assert_eq!(
            session.first_fault,
            original.or(Some(if invalid_number {
                "KEEPER_CONTROL"
            } else {
                "KEEPER_CHANNEL"
            }))
        );
        assert_eq!(result.unwrap_err().to_string(), "KEEPER_CLEANUP_DEADLINE");
        assert!(!session.gone && !session.released);
        assert!(channel.stream.output.is_empty());
    }
    #[test]
    fn r21_wrong_nonce_before_accepted_minimum_does_not_authorize_a_group_step() {
        r21_rejected_prefix(false, false);
    }
    #[test]
    fn r21_prior_expiry_fault_survives_rejected_line_and_buffered_minimum() {
        r21_rejected_prefix(true, false);
    }
    #[test]
    fn r21_invalid_number_before_valid_minimum_preserves_control_fault() {
        r21_rejected_prefix(false, true);
    }

    fn r21_fatal_channel(mut channel: Channel<ScriptedIo>) {
        let mut session = helper_fixture();
        session.control("CLEAN|1000000000", 1).unwrap();
        session.step(&mut || Ok(2)).unwrap();
        let mut expected = session.group.events.clone();
        let grace = session.term_end;
        let result = keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || {
            Ok(150_000_000)
        });
        assert!(
            result.is_err(),
            "unrecoverable transport must stop group authority rather than claim drain"
        );
        expected.push("disable");
        assert_eq!(session.group.events, expected);
        assert_eq!(session.first_fault, Some("KEEPER_CHANNEL"));
        assert_eq!(session.term_end, grace);
        assert!(!session.gone && !session.released);
        assert!(channel.stream.output.is_empty());
        // Even an explicit later call with valid bytes cannot regain authority.
        channel.stream.actions.push_back(ReadAction::Bytes(
            format!("{}|CLEAN|100\n", "a".repeat(32)).into_bytes(),
        ));
        assert!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_001
            ))
            .is_err()
        );
        assert!(session.step(&mut || Ok(150_000_001)).is_err());
        assert_eq!(session.group.events, expected);
    }
    #[test]
    fn r21_fatal_read_error_stops_group_authority() {
        let mut channel = scripted_channel(&[]);
        channel.stream.actions.push_back(ReadAction::Fault);
        r21_fatal_channel(channel);
    }
    #[test]
    fn r21_wire_overflow_stops_group_authority() {
        let mut channel = scripted_channel(format!("{}|CLEAN|100\n", "a".repeat(32)).as_bytes());
        channel.wire = MAX_WIRE;
        r21_fatal_channel(channel);
    }
    #[test]
    fn r21_unterminated_frame_limit_stops_group_authority() {
        r21_fatal_channel(scripted_channel(&[b'x'; MAX_FRAME]));
    }
    #[test]
    fn r21_truncated_eof_stops_group_authority() {
        let mut channel = scripted_channel(format!("{}|CLEAN|", "a".repeat(32)).as_bytes());
        channel.stream.actions.push_back(ReadAction::Bytes(vec![]));
        r21_fatal_channel(channel);
    }
    #[test]
    fn r21_partial_wouldblock_is_not_control_quiescence() {
        let mut session = helper_fixture();
        session.control("CLEAN|1000000000", 1).unwrap();
        session.step(&mut || Ok(2)).unwrap();
        let events = session.group.events.clone();
        let mut channel = scripted_channel(format!("{}|CLEAN|", "a".repeat(32)).as_bytes());
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_000
            ))
            .unwrap(),
            Turn::Again
        );
        assert_eq!(session.group.events, events);
        assert!(!channel.pending.is_empty());
        channel
            .stream
            .actions
            .push_back(ReadAction::Bytes(b"100000000\n".to_vec()));
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_001
            ))
            .unwrap_err()
            .to_string(),
            "KEEPER_CLEANUP_DEADLINE"
        );
        assert_eq!(session.end, 100_000_000);
        assert_eq!(session.group.events, events);
    }
    #[test]
    fn r21_complete_rejected_line_alone_can_clean_after_proven_quiescence() {
        let mut session = helper_fixture();
        session.control("CLEAN|1000", 1).unwrap();
        session.step(&mut || Ok(2)).unwrap();
        let mut expected = session.group.events.clone();
        let mut channel = scripted_channel(format!("{}|PARK\n", "b".repeat(32)).as_bytes());
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(3)).unwrap(),
            Turn::Again
        );
        expected.push("query");
        assert_eq!(session.group.events, expected);
        assert_eq!(session.first_fault, Some("KEEPER_CHANNEL"));
        assert!(!session.gone && !session.released);
        assert!(channel.stream.output.is_empty());
    }
    #[test]
    fn r21_recoverable_line_errors_drain_suffix_under_the_same_frame_cap() {
        let nonce = "a".repeat(32);
        let variants = [
            b"missing-pipe\n".to_vec(),
            vec![255, b'\n'],
            format!("{nonce}|PARK\r\n").into_bytes(),
            {
                let mut b = vec![b'x'; 270];
                b.push(b'\n');
                b
            },
        ];
        for bad in variants {
            let mut session = helper_fixture();
            session.control("CLEAN|1000000000", 1).unwrap();
            session.step(&mut || Ok(2)).unwrap();
            let events = session.group.events.clone();
            let mut bytes = bad;
            bytes.extend(r21_parent_frame(100_000_000, 10_000_000));
            let mut channel = scripted_channel(&[]);
            // A complete oversized line can be rejected at its known boundary
            // when a short prefix plus a second bounded read includes newline.
            channel.stream.actions = bytes
                .chunks(200)
                .map(|b| ReadAction::Bytes(b.to_vec()))
                .collect();
            assert_eq!(
                keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                    150_000_000
                ))
                .unwrap_err()
                .to_string(),
                "KEEPER_CLEANUP_DEADLINE"
            );
            assert_eq!(session.end, 100_000_000);
            assert_eq!(session.group.events, events);
            assert_eq!(session.first_fault, Some("KEEPER_CHANNEL"));
            assert!(channel.stream.output.is_empty());
        }
    }
    #[test]
    fn r21_rejected_frame_cap_yields_before_buffered_minimum() {
        let mut session = helper_fixture();
        session.control("CLEAN|1000000000", 1).unwrap();
        session.step(&mut || Ok(2)).unwrap();
        let events = session.group.events.clone();
        let mut bytes = format!("{}|PARK\n", "b".repeat(32)).repeat(8).into_bytes();
        bytes.extend(r21_parent_frame(100_000_000, 10_000_000));
        let mut channel = scripted_channel(&bytes);
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_000
            ))
            .unwrap(),
            Turn::Again
        );
        assert_eq!(session.group.events, events);
        assert_eq!(session.first_fault, Some("KEEPER_CHANNEL"));
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_001
            ))
            .unwrap_err()
            .to_string(),
            "KEEPER_CLEANUP_DEADLINE"
        );
        assert_eq!(session.end, 100_000_000);
        assert_eq!(session.group.events, events);
    }
    #[test]
    fn r21_fatal_stop_preserves_prior_expiry_reason() {
        let mut session = helper_fixture();
        let at = session.lifetime.operation_end;
        session.step(&mut || Ok(at)).unwrap();
        let mut events = session.group.events.clone();
        let mut channel = scripted_channel(&[]);
        channel.stream.actions.push_back(ReadAction::Fault);
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(at + 1))
                .unwrap_err()
                .to_string(),
            "KEEPER_LIFETIME"
        );
        events.push("disable");
        assert_eq!(session.group.events, events);
        assert_eq!(session.first_fault, Some("KEEPER_LIFETIME"));
        assert!(!session.gone && !session.released);
    }
    #[test]
    fn r21_fatal_error_after_absence_never_reports_terminal_success() {
        let mut session = helper_fixture();
        session.group.absent = true;
        session.control("CLEAN|1000", 1).unwrap();
        assert!(session.step(&mut || Ok(2)).unwrap());
        let mut channel = scripted_channel(&[]);
        channel.stream.actions.push_back(ReadAction::Fault);
        assert!(keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(3)).is_err());
        assert!(session.gone);
        assert!(!session.released);
        assert!(session.authority_stopped);
        assert_eq!(session.first_fault, Some("KEEPER_CHANNEL"));
        assert!(channel.stream.output.is_empty());
        assert!(session.control("RELEASE", 4).is_err());
        assert!(session.park(&mut || Ok(4)).is_err());
    }
    // R22: no group side effect, including initial PARK and its self-kill
    // fallback, can precede the same turn's bounded control drain.
    fn r22_park_with_expired_minimum(fail_park: bool) {
        let mut session = helper_fixture();
        session.group.fail_park = fail_park;
        let mut bytes = format!("{}|PARK\n", "a".repeat(32)).into_bytes();
        bytes.extend(r21_parent_frame(100_000_000, 10_000_000));
        let mut channel = scripted_channel(&bytes);
        let result = keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || {
            Ok(150_000_000)
        });
        assert!(
            session.group.events.is_empty(),
            "PARK/self-kill must not precede the buffered minimum"
        );
        assert_eq!(session.end, 100_000_000);
        assert!(channel.stream.output.is_empty());
        assert_eq!(result.unwrap_err().to_string(), "KEEPER_CLEANUP_DEADLINE");
        assert!(!session.gone && !session.released && !session.abandoned);
    }
    #[test]
    fn r22_initial_park_consumes_expired_buffered_minimum_before_membership_change() {
        r22_park_with_expired_minimum(false);
    }
    #[test]
    fn r22_failed_initial_park_cannot_self_kill_before_buffered_minimum() {
        r22_park_with_expired_minimum(true);
    }
    fn r22_pending_park_channel() -> (KeeperSession<FakeGroup>, Channel<ScriptedIo>) {
        let session = helper_fixture();
        let bytes = format!("{}|PARK\n", "a".repeat(32)).into_bytes();
        (session, scripted_channel(&bytes))
    }
    #[test]
    fn r22_park_intent_survives_read_budget_yield_without_group_work() {
        let (mut session, mut channel) = r22_pending_park_channel();
        channel
            .stream
            .actions
            .extend((0..7).map(|_| ReadAction::Interrupted));
        channel
            .stream
            .actions
            .push_back(ReadAction::Bytes(r21_parent_frame(100_000_000, 10_000_000)));
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_000
            ))
            .unwrap(),
            Turn::Again
        );
        assert!(session.group.events.is_empty());
        assert!(channel.stream.output.is_empty());
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_001
            ))
            .unwrap_err()
            .to_string(),
            "KEEPER_CLEANUP_DEADLINE"
        );
        assert_eq!(session.end, 100_000_000);
        assert!(session.group.events.is_empty());
    }
    #[test]
    fn r22_park_intent_survives_partial_wouldblock_without_group_work() {
        let (mut session, mut channel) = r22_pending_park_channel();
        channel.stream.actions.push_back(ReadAction::Bytes(
            format!("{}|CLEAN|", "a".repeat(32)).into_bytes(),
        ));
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_000
            ))
            .unwrap(),
            Turn::Again
        );
        assert!(session.group.events.is_empty());
        assert!(channel.stream.output.is_empty());
        channel
            .stream
            .actions
            .push_back(ReadAction::Bytes(b"100000000\n".to_vec()));
        assert!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_001
            ))
            .is_err()
        );
        assert_eq!(session.end, 100_000_000);
        assert!(session.group.events.is_empty());
    }
    #[test]
    fn r22_park_intent_is_not_executed_before_fatal_transport_stop() {
        let (mut session, mut channel) = r22_pending_park_channel();
        channel.stream.actions.push_back(ReadAction::Fault);
        assert!(keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(1)).is_err());
        assert_eq!(session.group.events, ["disable"]);
        assert!(channel.stream.output.is_empty());
        assert!(session.authority_stopped);
        assert_eq!(session.first_fault, Some("KEEPER_CHANNEL"));
    }
    #[test]
    fn r22_duplicate_pending_park_cannot_bypass_minimum_on_failure_cleanup() {
        let mut session = helper_fixture();
        let mut bytes = format!("{0}|PARK\n{0}|PARK\n", "a".repeat(32)).into_bytes();
        bytes.extend(r21_parent_frame(100_000_000, 10_000_000));
        let mut channel = scripted_channel(&bytes);
        assert!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_000
            ))
            .is_err()
        );
        assert!(session.group.events.is_empty());
        assert!(channel.stream.output.is_empty());
        assert_eq!(session.end, 100_000_000);
        assert_eq!(session.first_fault, Some("KEEPER_CONTROL"));
    }
    #[test]
    fn r22_timely_lone_park_preserves_original_handshake() {
        let (mut session, mut channel) = r22_pending_park_channel();
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(1)).unwrap(),
            Turn::Again
        );
        assert_eq!(session.group.events, ["park"]);
        assert!(session.parked);
        assert_eq!(
            channel.stream.output,
            format!("{}|PARKED|42|7|7\n", "a".repeat(32)).as_bytes()
        );
        assert!(!session.cleaning && !session.gone && session.first_fault.is_none());
    }
    #[test]
    fn r22_pre_park_eof_retains_failure_cleanup_and_parking_loss_fallback() {
        for fail_park in [false, true] {
            let mut session = helper_fixture();
            session.group.fail_park = fail_park;
            let mut channel = scripted_channel(&[]);
            channel.stream.actions.push_back(ReadAction::Bytes(vec![]));
            let result = keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(1));
            assert_eq!(session.first_fault, Some("KEEPER_OWNER_EOF"));
            assert!(!session.gone && !session.released);
            assert!(channel.stream.output.is_empty());
            if fail_park {
                assert!(result.is_err());
                assert_eq!(session.group.events, ["park", "self-kill"]);
            } else {
                assert_eq!(result.unwrap(), Turn::Again);
                assert_eq!(session.group.events, ["park", "query", "term"]);
            }
        }
    }
    #[test]
    fn r22_pending_park_survives_frame_cap_before_ninth_minimum() {
        let (mut session, mut channel) = r22_pending_park_channel();
        let mut rest = (0..7)
            .map(|n| format!("{}|CLEAN|{}\n", "a".repeat(32), 1_000_000_000 - n))
            .collect::<String>()
            .into_bytes();
        rest.extend(r21_parent_frame(100_000_000, 10_000_000));
        channel.stream.actions.extend(
            rest.chunks(MAX_FRAME)
                .map(|b| ReadAction::Bytes(b.to_vec())),
        );
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_000
            ))
            .unwrap(),
            Turn::Again
        );
        assert!(session.park_requested);
        assert!(session.group.events.is_empty());
        assert!(channel.stream.output.is_empty());
        assert!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_001
            ))
            .is_err()
        );
        assert_eq!(session.end, 100_000_000);
        assert!(session.group.events.is_empty());
    }
    #[test]
    fn r22_pending_park_is_executed_once_after_later_true_quiescence() {
        let (mut session, mut channel) = r22_pending_park_channel();
        channel
            .stream
            .actions
            .extend((0..8).map(|_| ReadAction::Interrupted));
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(1)).unwrap(),
            Turn::Again
        );
        assert!(session.park_requested);
        assert!(session.group.events.is_empty());
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(2)).unwrap(),
            Turn::Again
        );
        assert!(!session.park_requested);
        assert_eq!(session.group.events, ["park"]);
        let output = channel.stream.output.clone();
        assert_eq!(
            output,
            format!("{}|PARKED|42|7|7\n", "a".repeat(32)).as_bytes()
        );
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(3)).unwrap(),
            Turn::Again
        );
        assert_eq!(channel.stream.output, output);
        assert_eq!(session.group.events, ["park"]);
    }
    #[test]
    fn r22_parked_response_failure_still_starts_bounded_failure_cleanup() {
        let (mut session, mut channel) = r22_pending_park_channel();
        let bytes = channel
            .stream
            .actions
            .front()
            .map(|a| match a {
                ReadAction::Bytes(b) => b.len(),
                _ => 0,
            })
            .unwrap();
        channel.wire = MAX_WIRE - bytes;
        assert_eq!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(1)).unwrap(),
            Turn::Again
        );
        assert_eq!(session.group.events, ["park", "query", "term"]);
        assert_eq!(session.first_fault, Some("KEEPER_CONTROL"));
        assert!(!session.gone && !session.released);
        assert!(channel.stream.output.is_empty());
        assert!(session.end <= 1 + CLEANUP_NS);
    }
    #[test]
    fn r22_timely_park_loss_retains_one_failure_only_self_group_fallback() {
        let (mut session, mut channel) = r22_pending_park_channel();
        session.group.fail_park = true;
        assert!(keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(1)).is_err());
        assert_eq!(session.group.events, ["park", "self-kill"]);
        assert_eq!(session.first_fault, Some("KEEPER_PARK"));
        assert!(session.abandoned);
        assert!(!session.gone && !session.released);
        assert!(channel.stream.output.is_empty());
    }
    #[test]
    fn r22_rejected_line_after_park_intent_does_not_bypass_buffered_minimum() {
        let (mut session, mut channel) = r22_pending_park_channel();
        let mut bytes = format!("{}|PARK\n", "b".repeat(32)).into_bytes();
        bytes.extend(r21_parent_frame(100_000_000, 10_000_000));
        channel.stream.actions.push_back(ReadAction::Bytes(bytes));
        assert!(
            keeper_turn(&mut session, &mut channel, 42, 7, 7, &mut || Ok(
                150_000_000
            ))
            .is_err()
        );
        assert_eq!(session.end, 100_000_000);
        assert_eq!(session.first_fault, Some("KEEPER_CHANNEL"));
        assert!(session.group.events.is_empty());
        assert!(channel.stream.output.is_empty());
    }
}

// Short-policy acceptance runs the actual lifetime, settlement and keeper
// machines with finite clocks/socketpairs/GroupOps; no second cleanup model.
#[cfg(test)]
mod short_policy_tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    struct Group {
        actions: Vec<&'static str>,
        absent: bool,
    }
    impl GroupOps for Group {
        fn park(&mut self) -> io::Result<()> {
            self.actions.push("park");
            Ok(())
        }
        fn absent(&mut self) -> io::Result<bool> {
            self.actions.push("query");
            Ok(self.absent)
        }
        fn signal(&mut self, signal: i32) -> io::Result<()> {
            self.actions.push(if signal == libc::SIGTERM {
                "term"
            } else {
                "kill"
            });
            Ok(())
        }
        fn abandon_self_group(&mut self) -> io::Result<()> {
            panic!("not a failure parking case")
        }
        fn disable(&mut self) {
            self.actions.push("disable");
        }
    }
    fn session() -> KeeperSession<Group> {
        KeeperSession::new(
            Group {
                actions: vec![],
                absent: false,
            },
            Lifetime::with_policy(0, 550_000_000, CleanupPolicy::HookShort).unwrap(),
        )
    }
    #[test]
    fn admitted_hook_lifetime_reserves_fifty_ms_inside_original_end() {
        let lifetime = Lifetime::with_policy(0, 550_000_000, CleanupPolicy::HookShort).unwrap();
        assert_eq!(lifetime.operation_end, 500_000_000);
        assert_eq!(lifetime.hard_end, 550_000_000);
        assert_eq!(lifetime.policy, CleanupPolicy::HookShort);
        assert!(Lifetime::with_policy(0, 50_000_000, CleanupPolicy::HookShort).is_err());
        assert!(Lifetime::with_policy(1, 0, CleanupPolicy::HookShort).is_err());
    }
    #[test]
    fn parent_clean_is_capped_to_the_short_reserve_not_legacy_tail() {
        let (left, _right) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(left, "a".repeat(32)).unwrap();
        let mut state = Settlement::with_policy(550_000_000, CleanupPolicy::HookShort);
        state.begin(550_000_000, 100_000_000, &mut channel).unwrap();
        assert_eq!(state.progress.deadline_ns, 150_000_000);
        state.begin(500_000_000, 110_000_000, &mut channel).unwrap();
        assert_eq!(state.progress.deadline_ns, 150_000_000);
        state.begin(140_000_000, 120_000_000, &mut channel).unwrap();
        assert_eq!(state.progress.deadline_ns, 140_000_000);
    }
    #[test]
    fn keeper_failure_and_control_share_the_same_short_minimum() {
        let mut s = session();
        s.fail("ORIGINAL", 100_000_000);
        assert_eq!(s.end, 150_000_000);
        s.control("CLEAN|140000000", 110_000_000).unwrap();
        s.control("CLEAN|500000000", 120_000_000).unwrap();
        assert_eq!(s.end, 140_000_000);
        assert_eq!(s.first_fault, Some("ORIGINAL"));
    }
    #[test]
    fn expiry_starts_cleanup_inside_end_and_signals_never_after_end() {
        let mut s = session();
        s.parked = true;
        s.step(&mut || Ok(500_000_000)).unwrap();
        assert_eq!(s.end, 550_000_000);
        assert_eq!(s.term_end, Some(515_000_000));
        assert_eq!(s.group.actions, ["query", "term"]);
        s.step(&mut || Ok(515_000_000)).unwrap();
        assert_eq!(s.group.actions, ["query", "term", "query", "kill"]);
        let before = s.group.actions.clone();
        assert!(s.step(&mut || Ok(550_000_000)).is_err());
        assert_eq!(s.group.actions, before);
        assert_eq!(s.first_fault, Some("KEEPER_LIFETIME"));
    }
    #[test]
    fn short_handshake_and_failed_drop_never_restart_the_inherited_budget() {
        let p = CleanupPolicy::HookShort;
        assert_eq!(p.setup_end(100_000_000, 550_000_000), 120_000_000);
        assert_eq!(p.setup_end(490_000_000, 550_000_000), 500_000_000);
        assert_eq!(p.observation_end(100_000_000, 140_000_000), 140_000_000);
        assert_eq!(p.observation_end(200_000_000, 140_000_000), 140_000_000);
        assert_eq!(p.observation_end(u64::MAX - 1, u64::MAX), u64::MAX);
        assert_eq!(
            CleanupPolicy::Legacy.setup_end(0, 3_000_000_000),
            1_200_000_000
        );
    }
    #[test]
    fn faulted_parent_caps_retention_to_short_end_without_clearing_first_fault() {
        let (left, _right) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(left, "a".repeat(32)).unwrap();
        let mut state = Settlement::with_policy(550_000_000, CleanupPolicy::HookShort);
        state.progress.fail("ORIGINAL");
        assert!(state.begin(550_000_000, 100_000_000, &mut channel).is_err());
        state.cap_failed_observation(Some(100_000_000), 550_000_000);
        assert_eq!(state.progress.deadline_ns, 150_000_000);
        state.cap_failed_observation(Some(110_000_000), 500_000_000);
        assert_eq!(state.progress.deadline_ns, 150_000_000);
        state.cap_failed_observation(None, 500_000_000);
        assert_eq!(state.progress.deadline_ns, 0);
        assert_eq!(state.progress.first_fault, Some("ORIGINAL"));
    }
    #[test]
    fn failed_control_send_keeps_failed_observation_within_same_short_policy() {
        let (left, right) = UnixStream::pair().unwrap();
        left.shutdown(std::net::Shutdown::Write).unwrap();
        let mut channel = Channel::new(left, "a".repeat(32)).unwrap();
        let mut state = Settlement::with_policy(550_000_000, CleanupPolicy::HookShort);
        assert!(state.begin(550_000_000, 100_000_000, &mut channel).is_err());
        state.progress.fail("SEND_FAILED");
        state.cap_failed_observation(Some(100_000_000), 550_000_000);
        assert_eq!(state.progress.deadline_ns, 150_000_000);
        assert!(!state.cleanup_sent);
        assert_eq!(state.progress.first_fault, Some("SEND_FAILED"));
        drop(right);
    }
    #[test]
    fn keeper_policy_tag_is_closed_not_a_caller_supplied_allowance() {
        assert_eq!(
            CleanupPolicy::keeper_tag(None).unwrap(),
            CleanupPolicy::Legacy
        );
        assert_eq!(
            CleanupPolicy::keeper_tag(Some("hook-short-v1")).unwrap(),
            CleanupPolicy::HookShort
        );
        for invalid in [
            "",
            "legacy",
            "hook-short-v2",
            "5000000000",
            "hook-short-v1\n",
        ] {
            assert_eq!(
                CleanupPolicy::keeper_tag(Some(invalid))
                    .unwrap_err()
                    .to_string(),
                "KEEPER_POLICY"
            );
        }
    }
    #[test]
    fn legacy_policy_values_and_admission_remain_unchanged() {
        let life = Lifetime::new(0, 3_000_000_000).unwrap();
        assert_eq!(life.policy, CleanupPolicy::Legacy);
        assert_eq!(life.operation_end, 1_200_000_000);
        assert!(Lifetime::new(0, CLEANUP_NS).is_err());
        assert_eq!(CleanupPolicy::Legacy.reserve_ns(), CLEANUP_NS);
        assert_eq!(CleanupPolicy::Legacy.term_ns(), 15_000_000);
        assert_eq!(CleanupPolicy::HookShort.reserve_ns(), 50_000_000);
        assert_eq!(CleanupPolicy::HookShort.term_ns(), 15_000_000);
        assert_eq!(CleanupPolicy::HookShort.setup_ns(), 20_000_000);
    }
}
