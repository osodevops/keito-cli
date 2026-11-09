//! Development-only reserved group cleanup. Not connected to product commands.
//!
//! The keeper is a sibling, never a wrapper/ancestor of the workload. Only the
//! keeper queries or signals its own reserved PGID. An opaque same-session tool
//! may use this lease; the guardian's setsid core must be a cooperative owner.
use crate::source_qa_protocol::{ProtocolError, SharedOwner};
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
}
impl Progress {
    fn new(deadline_ns: u64) -> Self {
        Self {
            phase: Phase::AwaitReady,
            first_fault: None,
            deadline_ns,
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
    fn shorten(&mut self, deadline_ns: u64) {
        self.deadline_ns = self.deadline_ns.min(deadline_ns);
    }
    fn fail(&mut self, reason: &'static str) {
        self.first_fault.get_or_insert(reason);
        self.phase = Phase::Failed;
    }
    #[cfg(test)]
    fn may_touch_group(&self) -> bool {
        matches!(self.phase, Phase::Parked | Phase::Cleaning) && self.first_fault.is_none()
    }
}

/// Separate closed keeper protocol. No paths/content/auth/capture fields.
struct Channel {
    stream: UnixStream,
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
        if self.stream.write(bytes.as_bytes())? != bytes.len() {
            return Err(refused("GROUP_SHORT_WRITE"));
        }
        Ok(())
    }
    fn receive(&mut self) -> io::Result<Option<String>> {
        for _ in 0..8 {
            if let Some(end) = self.pending.iter().position(|v| *v == b'\n') {
                let line: Vec<_> = self.pending.drain(..=end).collect();
                if line.len() > MAX_FRAME || !line.is_ascii() {
                    return Err(refused("GROUP_FRAME"));
                }
                let text = std::str::from_utf8(&line[..line.len() - 1])
                    .map_err(|_| refused("GROUP_FRAME"))?;
                let (nonce, body) = text.split_once('|').ok_or_else(|| refused("GROUP_FRAME"))?;
                if nonce != self.nonce || body.contains('\r') {
                    return Err(refused("GROUP_NONCE"));
                }
                return Ok(Some(body.to_owned()));
            }
            if self.pending.len() >= MAX_FRAME {
                return Err(refused("GROUP_FRAME_LIMIT"));
            }
            if self.eof {
                return Ok(None);
            }
            let mut bytes = [0u8; MAX_FRAME];
            match self.stream.read(&mut bytes) {
                Ok(0) => {
                    self.eof = true;
                    if !self.pending.is_empty() {
                        return Err(refused("GROUP_TRUNCATED"));
                    }
                    return Ok(None);
                }
                Ok(n) => {
                    self.charge(n)?;
                    self.pending.extend_from_slice(&bytes[..n]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(refused("GROUP_READ")),
            }
        }
        Ok(None)
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

/// Actual direct wait facts, separate from the keeper's group-absence evidence.
pub struct GroupProof {
    pub workload_status: Option<ExitStatus>,
    pub keeper_status: ExitStatus,
    pub group_absent: bool,
}

/// Parent must retain this object in its local registry BEFORE calling start or
/// publishing receipts. Errors retain both handles here; no observer registry is
/// yet wired to product commands. Drop is failure cleanup, never a clean proof.
pub struct GroupLease {
    keeper: Child,
    workload: Option<Child>,
    workload_status: Option<ExitStatus>,
    keeper_status: Option<ExitStatus>,
    channel: Channel,
    progress: Progress,
    owner: SharedOwner,
    keeper_pid: libc::pid_t,
    owner_pgid: libc::pid_t,
    sid: libc::pid_t,
    cleanup_sent: bool,
    release_sent: bool,
}
impl GroupLease {
    pub fn prepare(
        owner: SharedOwner,
        run_root: &Path,
        keeper_image: &Path,
        lifetime_deadline_ns: u64,
    ) -> io::Result<Self> {
        owner
            .borrow()
            .check_admission()
            .map_err(|_| refused("GROUP_ADMISSION"))?;
        let current = now()?;
        if lifetime_deadline_ns <= current || lifetime_deadline_ns - current > MAX_LIFETIME_NS {
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
        let mut lease = Self {
            keeper,
            workload: None,
            workload_status: None,
            keeper_status: None,
            channel,
            progress: Progress::new(lifetime_deadline_ns),
            owner,
            keeper_pid,
            owner_pgid,
            sid,
            cleanup_sent: false,
            release_sent: false,
        };
        drop(inherited);
        drop(child_end);
        let ready = lease
            .channel
            .wait_frame(current.saturating_add(CLEANUP_NS).min(lifetime_deadline_ns));
        match ready {
            Ok(body) if body == format!("READY|{keeper_pid}|{keeper_pid}|{sid}") => {
                lease.progress.advance(Phase::AwaitReady, Phase::Ready)?;
                lease.check_admission()?;
                Ok(lease)
            }
            _ => {
                lease.fail("GROUP_READY");
                Err(refused("GROUP_READY"))
            }
        }
    }
    fn fail(&mut self, reason: &'static str) {
        self.progress.fail(reason);
        self.owner.borrow_mut().poison(ProtocolError(reason));
    }
    fn check_admission(&mut self) -> io::Result<()> {
        if self.owner.borrow().check_admission().is_err() {
            self.fail("GROUP_ADMISSION");
            return Err(refused("GROUP_ADMISSION"));
        }
        Ok(())
    }
    pub fn start(&mut self, command: &mut Command) -> io::Result<()> {
        let result = self.start_inner(command);
        if result.is_err() {
            self.fail("GROUP_START_FAILED");
        }
        result
    }
    fn start_inner(&mut self, command: &mut Command) -> io::Result<()> {
        self.check_admission()?;
        if self.progress.phase != Phase::Ready || self.workload.is_some() {
            self.fail("GROUP_START_STATE");
            return Err(refused("GROUP_START_STATE"));
        }
        let pgid = self.keeper_pid;
        let sid = self.sid;
        // Only append libc operations. The original prepared command's fd/image
        // checks, argv, cwd and environment remain unchanged. setsid commands
        // are intentionally refused: this lease is for same-session tools.
        unsafe {
            command.pre_exec(move || {
                if libc::getsid(0) != sid || libc::setpgid(0, pgid) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        self.workload = Some(child);
        self.progress.advance(Phase::Ready, Phase::Joined)?;
        self.check_admission()?;
        self.channel.send("PARK")?;
        let response = self.channel.wait_frame(
            now()?
                .saturating_add(CLEANUP_NS)
                .min(self.progress.deadline_ns),
        )?;
        if response
            != format!(
                "PARKED|{}|{}|{}",
                self.keeper_pid, self.owner_pgid, self.sid
            )
        {
            self.fail("GROUP_PARK");
            return Err(refused("GROUP_PARK"));
        }
        self.progress.advance(Phase::Joined, Phase::Parked)
    }
    pub fn stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.workload.as_mut()?.stdin.take()
    }
    pub fn stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.workload.as_mut()?.stdout.take()
    }
    pub fn stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.workload.as_mut()?.stderr.take()
    }
    /// Broadcast cleanup before waiting; another owner's earlier absolute
    /// deadline may only shorten this lease. No signals are sent by the parent.
    pub fn begin_cleanup(&mut self, deadline_ns: u64) -> io::Result<()> {
        let result = self.begin_cleanup_inner(deadline_ns);
        if result.is_err() {
            self.fail("GROUP_CLEANUP_BEGIN_FAILED");
        }
        result
    }
    fn begin_cleanup_inner(&mut self, deadline_ns: u64) -> io::Result<()> {
        self.owner.borrow_mut().admission_closed = true;
        self.progress
            .shorten(deadline_ns.min(now()?.saturating_add(CLEANUP_NS)));
        if self.cleanup_sent {
            return Ok(());
        }
        self.cleanup_sent = true;
        if self.progress.first_fault.is_none() {
            self.progress.phase = Phase::Cleaning;
        }
        if let Some(child) = self.workload.as_mut() {
            child.stdin.take();
        }
        self.channel
            .send(&format!("CLEAN|{}", self.progress.deadline_ns))
    }
    pub fn poll_cleanup(&mut self) -> io::Result<Option<GroupProof>> {
        let result = self.poll_inner();
        if result.is_err() {
            self.fail("GROUP_CLEANUP_UNPROVEN");
        }
        result
    }
    fn poll_inner(&mut self) -> io::Result<Option<GroupProof>> {
        if self.progress.first_fault.is_some() {
            return Err(refused("GROUP_PREVIOUS_FAULT"));
        }
        if !self.cleanup_sent || now()? >= self.progress.deadline_ns {
            return Err(refused("GROUP_CLEANUP_DEADLINE"));
        }
        if self.workload_status.is_none() {
            if let Some(child) = self.workload.as_mut() {
                self.workload_status = child.try_wait()?;
            }
        }
        if let Some(body) = self.channel.receive()? {
            if body != format!("GONE|{}", self.keeper_pid) || self.release_sent {
                return Err(refused("GROUP_TERMINAL"));
            }
            // The keeper has irreversibly disabled every group operation before
            // this frame. A later keeper wait cannot authorize another signal.
            self.progress.phase = Phase::Gone;
        }
        if self.progress.phase == Phase::Gone
            && (self.workload.is_none() || self.workload_status.is_some())
            && !self.release_sent
        {
            self.channel.send("RELEASE")?;
            self.channel.stream.shutdown(std::net::Shutdown::Write)?;
            self.release_sent = true;
        }
        if self.release_sent && self.keeper_status.is_none() {
            self.keeper_status = self.keeper.try_wait()?;
        }
        if let Some(status) = self.keeper_status {
            if !status.success()
                || !self.channel.eof
                || !self.channel.pending.is_empty()
                || self.progress.first_fault.is_some()
            {
                if !status.success() || self.progress.first_fault.is_some() {
                    return Err(refused("GROUP_KEEPER_FAILED"));
                }
                return Ok(None);
            }
            self.progress.phase = Phase::Released;
            return Ok(Some(GroupProof {
                workload_status: self.workload_status,
                keeper_status: status,
                group_absent: true,
            }));
        }
        Ok(None)
    }
}
impl Drop for GroupLease {
    fn drop(&mut self) {
        if self.progress.phase == Phase::Released {
            return;
        }
        self.owner
            .borrow_mut()
            .poison(ProtocolError("GROUP_DROPPED_WITHOUT_PROOF"));
        // Close the lifeline: the finite keeper must clean its own reserved
        // group. Do not signal a cached group from this parent after any fault.
        let _ = self.channel.stream.shutdown(std::net::Shutdown::Both);
        let deadline = now()
            .unwrap_or(0)
            .saturating_add(CLEANUP_NS)
            .min(self.progress.deadline_ns);
        while now().unwrap_or(deadline) < deadline {
            if self.workload_status.is_none() {
                if let Some(child) = self.workload.as_mut() {
                    self.workload_status = child.try_wait().ok().flatten();
                }
            }
            if self.keeper_status.is_none() {
                self.keeper_status = self.keeper.try_wait().ok().flatten();
            }
            if self.keeper_status.is_some()
                && (self.workload.is_none() || self.workload_status.is_some())
            {
                break;
            }
            pause();
        }
        // A deadline/unknown kernel wait remains failed; never label dropped
        // or unreaped handles clean. Full Scope registry integration is pending.
    }
}

struct KeeperGroup {
    pid: libc::pid_t,
    park: libc::pid_t,
    parked: bool,
    no_more_group_ops: bool,
    cleanup_started: bool,
}
impl KeeperGroup {
    fn park(&mut self) -> io::Result<()> {
        if !self.parked {
            if unsafe { libc::setpgid(0, self.park) } != 0 {
                return Err(refused("KEEPER_PARK"));
            }
            self.parked = true;
        }
        Ok(())
    }
    fn absent(&self) -> io::Result<bool> {
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
    fn signal(&self, signal: libc::c_int) -> io::Result<()> {
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
    fn clean(&mut self, deadline: u64) -> io::Result<()> {
        if self.no_more_group_ops || self.cleanup_started {
            return Err(refused("KEEPER_CLEAN_TWICE"));
        }
        self.cleanup_started = true;
        self.park()?;
        if now()? >= deadline {
            return Err(refused("KEEPER_CLEANUP_DEADLINE"));
        }
        let term_end = now()?.saturating_add(TERM_NS).min(deadline);
        if !self.absent()? {
            self.signal(libc::SIGTERM)?;
        }
        while now()? < term_end {
            if self.absent()? {
                self.no_more_group_ops = true;
                return Ok(());
            }
            pause();
        }
        if !self.absent()? {
            self.signal(libc::SIGKILL)?;
        }
        while now()? < deadline {
            if self.absent()? {
                self.no_more_group_ops = true;
                return Ok(());
            }
            pause();
        }
        Err(refused("KEEPER_GROUP_NOT_ABSENT"))
    }
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
    if values.len() != 6 || !nonce_valid(&values[1]) {
        return Err(refused("KEEPER_ARGS"));
    }
    let fd = number(&values[0])?;
    let parent = number(&values[2])?;
    let park = number(&values[3])?;
    let sid = number(&values[4])?;
    let deadline = number(&values[5])?;
    if !(4..=1024).contains(&fd)
        || parent != unsafe { libc::getppid() } as u64
        || park == 0
        || park > i32::MAX as u64
        || sid != unsafe { libc::getsid(0) } as u64
        || unsafe { libc::getpid() } != unsafe { libc::getpgrp() }
        || unsafe { libc::getpid() } as u64 == sid
        || deadline <= now()?
        || deadline - now()? > MAX_LIFETIME_NS
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
    let mut group = KeeperGroup {
        pid: unsafe { libc::getpid() },
        park: park as libc::pid_t,
        parked: false,
        no_more_group_ops: false,
        cleanup_started: false,
    };
    let result = (|| {
        channel.send(&format!("READY|{}|{}|{}", group.pid, group.pid, sid))?;
        let mut cleaned = false;
        while now()? < deadline {
            if let Some(body) = channel.receive()? {
                if body == "PARK" && !group.parked && !cleaned {
                    group.park()?;
                    channel.send(&format!("PARKED|{}|{}|{}", group.pid, park, sid))?;
                } else if let Some(end) = body.strip_prefix("CLEAN|") {
                    if cleaned {
                        return Err(refused("KEEPER_CLEAN_TWICE"));
                    }
                    let end = number(end)?
                        .min(deadline)
                        .min(now()?.saturating_add(CLEANUP_NS));
                    group.clean(end)?;
                    cleaned = true;
                    channel.send(&format!("GONE|{}", group.pid))?;
                } else if body == "RELEASE" && cleaned {
                    // RELEASE is terminal only together with empty EOF. Do not
                    // ignore a duplicate/trailing frame after a valid release.
                    while now()? < deadline {
                        if channel.receive()?.is_some() {
                            return Err(refused("KEEPER_TRAILING_RELEASE"));
                        }
                        if channel.eof {
                            return Ok(());
                        }
                        pause();
                    }
                    return Err(refused("KEEPER_RELEASE_EOF_DEADLINE"));
                } else {
                    return Err(refused("KEEPER_STATE"));
                }
            }
            if channel.eof {
                return Err(refused("KEEPER_OWNER_EOF"));
            }
            pause();
        }
        Err(refused("KEEPER_LIFETIME"))
    })();
    if result.is_err() && !group.no_more_group_ops && !group.cleanup_started {
        // The keeper's own PID stays reserved throughout this attempt. A
        // failed emergency cleanup yields no GONE/clean proof to its parent.
        let _ = group.clean(now()?.saturating_add(CLEANUP_NS).min(deadline));
    }
    result
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
}
