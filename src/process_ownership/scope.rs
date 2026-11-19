//! Explicit entry-thread command owner. No implicit/TLS runner or positive
//! capture authority. Cooperative children require terminal, physical EOF and
//! retained direct waits; opaque leaves use the paired guard's reserved groups.
use super::{
    group::{CleanupPolicy, GroupLease},
    io::{CommandOutput, IoFacts, IoPlan, PipeIo},
    protocol::{Endpoint, ProtocolError, Registration, SharedOwner, MAX_REGISTRATIONS},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            net::UnixStream,
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    rc::Rc,
};

const RESERVE_NS: u64 = 50_000_000;
const MAX_IO_BYTES: usize = 4 * 1024 * 1024;
const SLOTS_PER_TURN: usize = 8;
/// Original maximum for one selected installation; an earlier parent always wins.
pub const INSTALL_OPERATION_NS: u64 = 30_000_000_000;
const ORDINARY_OPERATION_NS: u64 = 1_000_000_000;

// The fixed developer version/help lane has the ordinary one-second owner.
// Keep the selector closed; callers cannot supply a keeper policy or duration.
#[cfg(feature = "dev-tools")]
pub(super) fn dev_client_keeper_policy() -> CleanupPolicy {
    CleanupPolicy::CommandProbe
}

fn error(reason: &'static str) -> io::Error {
    io::Error::other(reason)
}
fn spawn_cooperative_before<T>(
    hard_end: u64,
    admission_ceiling: Option<u64>,
    fresh: u64,
    spawn: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let admission_end = admission_ceiling
        .unwrap_or(hard_end)
        .min(hard_end)
        .saturating_sub(RESERVE_NS);
    if fresh >= admission_end || super::signals::pending() {
        return Err(error("SCOPE_ADMISSION"));
    }
    spawn()
}
fn clear_parent_environment(command: &mut Command) {
    for key in [
        "KEITO_PROCESS_OWNER_FD",
        "KEITO_PROCESS_OWNER_NONCE",
        "KEITO_PROCESS_OWNER_PID",
        "KEITO_PROCESS_OWNER_END_NS",
        "KEITO_PROCESS_OWNER_CONTRACT",
        "KEITO_SOURCE_OWNER_FD",
        "KEITO_SOURCE_OWNER_NONCE",
        "KEITO_CAPTURE_ACK_FD",
        "KEITO_CAPTURE_IMAGE_FD",
        "KEITO_CAPTURE_INVOCATION_NONCE",
        "KEITO_CAPTURE_LEAF_END_NS",
        "KEITO_CAPTURE_KEEPER_IMAGE_FD",
        "KEITO_CAPTURE_CORE_SHA256",
    ] {
        command.env_remove(key);
    }
}
fn now() -> io::Result<u64> {
    let mut clock = std::mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, clock.as_mut_ptr()) } != 0 {
        return Err(error("SCOPE_CLOCK"));
    }
    let clock = unsafe { clock.assume_init() };
    u64::try_from(clock.tv_sec)
        .ok()
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|s| s.checked_add(clock.tv_nsec as u64))
        .ok_or_else(|| error("SCOPE_CLOCK"))
}
pub(crate) fn monotonic_now() -> io::Result<u64> {
    now()
}

#[derive(Clone)]
pub struct SlotId {
    index: usize,
    scope: Rc<()>,
}
#[derive(Clone, Copy, Debug, Default)]
struct EdgeFacts {
    status: Option<ExitStatus>,
    waits_known: bool,
    group_absent: bool,
    terminal_eof: bool,
    resource_proof: bool,
    io_required: bool,
    io: IoFacts,
    first_fault: Option<&'static str>,
}
impl EdgeFacts {
    fn resources_complete(&self) -> bool {
        self.resource_proof && self.waits_known && (!self.io_required || self.io.complete())
    }
}
trait Edge {
    fn begin(&mut self, end: u64) -> io::Result<()>;
    fn poll(&mut self) -> io::Result<()>;
    fn facts(&self) -> EdgeFacts;
    #[cfg(feature = "capture-profile")]
    fn repository_profile_stage(&self) -> Option<crate::capture_profile::Phase> {
        None
    }
    fn setup_pending(&self) -> bool {
        false
    }
    fn setup_wait_source(&self) -> Option<(i32, u64)> {
        None
    }
    fn progress_wait_sources(&self, _descriptors: &mut Vec<libc::pollfd>) -> Option<u64> {
        None
    }
    fn progress_buffered_event(&self) -> bool {
        false
    }
    fn setup_buffered_event(&self) -> bool {
        false
    }
    fn post_join_pending(&self) -> bool {
        false
    }
    #[cfg(feature = "dev-tools")]
    fn setup_trace(&self) -> Option<super::group::SetupTrace> {
        None
    }
    fn take_output(&mut self) -> CommandOutput;
    fn take_events(&mut self) -> Vec<String> {
        Vec::new()
    }
}
enum Stage {
    Ready,
    Park,
    Running,
    Cleaning,
    Done,
}
impl Stage {
    #[cfg(feature = "capture-profile")]
    fn repository_profile_stage(&self) -> crate::capture_profile::Phase {
        use crate::capture_profile::Phase;
        match self {
            Self::Ready => Phase::RepoReadyPoll,
            Self::Park => Phase::RepoParkPoll,
            Self::Running => Phase::RepoRunningPoll,
            Self::Cleaning => Phase::RepoCleaningPoll,
            Self::Done => Phase::RepoDonePoll,
        }
    }
    fn setup_pending(&self) -> bool {
        matches!(self, Self::Ready | Self::Park)
    }
}
// Scheduling only: this lock never proves command identity, consent or success.
// Retain each slot through group and pipe finality, so a parked command cannot
// admit another live Git group into the same four-slot pool. This is not an
// absolute process bound after owner death or unproved cleanup.
// Each admission opens an independent description; never duplicate the lock FD.
const HOOK_SETUP_SLOTS: usize = 4;
#[derive(Clone, Copy)]
enum HookCapacityKind {
    Leaf,
    Core,
}
struct HookSetupAdmission {
    file: File,
    name: &'static [u8],
}
impl HookSetupAdmission {
    #[cfg(test)]
    fn open(directory: &File, slot: usize) -> io::Result<Self> {
        Self::open_kind(directory, slot, HookCapacityKind::Leaf)
    }
    fn open_kind(directory: &File, slot: usize, kind: HookCapacityKind) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let parent = directory
            .metadata()
            .map_err(|_| error("SCOPE_SETUP_GATE"))?;
        if !parent.is_dir()
            || parent.uid() != unsafe { libc::geteuid() }
            || parent.mode() & 0o777 != 0o700
        {
            return Err(error("SCOPE_SETUP_GATE"));
        }
        let name: &'static [u8] = match (kind, slot) {
            (HookCapacityKind::Leaf, 0) => b"capture-hook-setup-0.lock\0",
            (HookCapacityKind::Leaf, 1) => b"capture-hook-setup-1.lock\0",
            (HookCapacityKind::Leaf, 2) => b"capture-hook-setup-2.lock\0",
            (HookCapacityKind::Leaf, 3) => b"capture-hook-setup-3.lock\0",
            (HookCapacityKind::Core, 0) => b"capture-hook-core-0.lock\0",
            (HookCapacityKind::Core, 1) => b"capture-hook-core-1.lock\0",
            (HookCapacityKind::Core, 2) => b"capture-hook-core-2.lock\0",
            (HookCapacityKind::Core, 3) => b"capture-hook-core-3.lock\0",
            _ => return Err(error("SCOPE_SETUP_GATE")),
        };
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr().cast(),
                libc::O_RDWR
                    | libc::O_CREAT
                    | libc::O_NONBLOCK
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(error("SCOPE_SETUP_GATE"));
        }
        let admission = Self {
            file: unsafe { File::from_raw_fd(fd) },
            name,
        };
        admission.check(directory)?;
        Ok(admission)
    }
    fn check(&self, directory: &File) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let metadata = self
            .file
            .metadata()
            .map_err(|_| error("SCOPE_SETUP_GATE"))?;
        let name = self.name;
        let mut current = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr().cast(),
                current.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(error("SCOPE_SETUP_GATE"));
        }
        let current = unsafe { current.assume_init() };
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
            || metadata.nlink() != 1
            || metadata.len() != 0
            || current.st_dev as u128 != u128::from(metadata.dev())
            || current.st_ino as u128 != u128::from(metadata.ino())
            || current.st_mode & libc::S_IFMT != libc::S_IFREG
        {
            return Err(error("SCOPE_SETUP_GATE"));
        }
        Ok(())
    }
    fn try_lock(&mut self, directory: &File) -> io::Result<bool> {
        self.check(directory)?;
        if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            self.check(directory)?;
            return Ok(true);
        }
        if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(error("SCOPE_SETUP_GATE"))
        }
    }
}

// Guard-only scheduling ownership. It grants no capture authority. CLOEXEC and
// the guard's child descriptor inventory keep the lock in the owning guard.
// A guard retains it through its final Scope::finish and terminal publication;
// as with leaf permits, this is not an after-crash/unproved-cleanup bound.
#[allow(dead_code)] // Also compiled into the standalone guard from shared source.
pub(crate) struct HookCoreAdmission {
    _permit: HookSetupAdmission,
}

struct GroupEdge {
    lease: GroupLease,
    command: Option<Command>,
    pipes: PipeIo,
    stage: Stage,
    proof: bool,
    first_fault: Option<&'static str>,
    published: [bool; 4],
    setup_admission: Option<HookSetupAdmission>,
    post_join_pending: bool,
}
fn release_hook_admission(
    stage: &Stage,
    facts: EdgeFacts,
    admission: &mut Option<HookSetupAdmission>,
) {
    if matches!(stage, Stage::Done)
        && facts.resources_complete()
        && facts.group_absent
        && facts.terminal_eof
    {
        admission.take();
    }
}
// Attach all retained handles before propagating even a post-spawn error.
// Success schedules ONE later PARK observation after the upstream drain.
fn retain_group_start(
    stage: &mut Stage,
    pending: &mut bool,
    started: io::Result<()>,
    attach: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let attached = attach();
    *stage = Stage::Park;
    *pending = false;
    started?;
    attached?;
    *pending = true;
    Ok(())
}

impl GroupEdge {
    fn progress(&mut self) -> io::Result<()> {
        // Outputs progress even while PARK is pending. Each pipe has a fixed
        // per-turn byte/syscall cap, and no WouldBlock is accepted as EOF.
        self.pipes.poll();
        if let Some(reason) = self.pipes.facts().first_fault {
            self.first_fault.get_or_insert(reason);
            if !matches!(self.stage, Stage::Cleaning | Stage::Done) {
                return Err(error(reason));
            }
        }
        match self.stage {
            Stage::Ready => {
                if self.lease.poll_ready()? {
                    let command = self.command.take().ok_or_else(|| error("SCOPE_COMMAND"))?;
                    let started = self.lease.start_pending(command);
                    // Even an error AFTER spawn must transfer every available
                    // pipe to this retained slot before returning the fault.
                    let workload_spawned = self.lease.report().workload_spawned;
                    let lease = &mut self.lease;
                    let pipes = &mut self.pipes;
                    retain_group_start(
                        &mut self.stage,
                        &mut self.post_join_pending,
                        started,
                        || {
                            if workload_spawned {
                                pipes.attach(lease.stdin(), lease.stdout(), lease.stderr())
                            } else {
                                Ok(())
                            }
                        },
                    )?;
                }
            }
            Stage::Park => {
                self.post_join_pending = false;
                if self.lease.poll_parked()? {
                    self.stage = Stage::Running;
                }
            }
            Stage::Running => {
                self.lease.poll_workload()?;
            }
            Stage::Cleaning => {
                if self.lease.poll_local_cleanup()?.is_some() {
                    self.proof = true;
                    self.stage = Stage::Done;
                }
            }
            Stage::Done => {}
        }
        if self.setup_admission.is_some() && matches!(self.stage, Stage::Done) {
            let facts = self.facts();
            release_hook_admission(&self.stage, facts, &mut self.setup_admission);
        }
        Ok(())
    }
}
impl Edge for GroupEdge {
    fn progress_wait_sources(&self, descriptors: &mut Vec<libc::pollfd>) -> Option<u64> {
        if self.stage.setup_pending() {
            return None;
        }
        self.pipes.progress_wait_sources(descriptors);
        if matches!(self.stage, Stage::Cleaning) {
            if let Some(fd) = self.lease.cleanup_wait_source() {
                descriptors.push(libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
        }
        Some(self.lease.progress_wait_end())
    }
    fn progress_buffered_event(&self) -> bool {
        matches!(self.stage, Stage::Cleaning) && self.lease.cleanup_buffered_event()
    }
    #[cfg(feature = "capture-profile")]
    fn repository_profile_stage(&self) -> Option<crate::capture_profile::Phase> {
        Some(self.stage.repository_profile_stage())
    }
    #[cfg(feature = "dev-tools")]
    fn setup_trace(&self) -> Option<super::group::SetupTrace> {
        Some(self.lease.setup_trace())
    }
    fn setup_pending(&self) -> bool {
        self.stage.setup_pending()
    }
    fn setup_buffered_event(&self) -> bool {
        self.stage.setup_pending() && self.lease.setup_buffered_event()
    }
    fn post_join_pending(&self) -> bool {
        self.post_join_pending && matches!(self.stage, Stage::Park)
    }
    fn setup_wait_source(&self) -> Option<(i32, u64)> {
        self.stage
            .setup_pending()
            .then(|| self.lease.setup_wait_source())
    }
    fn begin(&mut self, end: u64) -> io::Result<()> {
        self.command.take();
        self.pipes.cancel_input();
        if matches!(self.stage, Stage::Done) {
            return Ok(());
        }
        self.stage = Stage::Cleaning;
        self.post_join_pending = false;
        self.lease.begin_local_cleanup(end)
    }
    fn poll(&mut self) -> io::Result<()> {
        #[cfg(feature = "capture-profile")]
        let result = {
            let _span = crate::capture_profile::Span::start(self.stage.repository_profile_stage());
            self.progress()
        };
        #[cfg(not(feature = "capture-profile"))]
        let result = self.progress();
        if result.is_err() {
            self.first_fault.get_or_insert(
                self.lease
                    .report()
                    .first_fault
                    .or(self.pipes.facts().first_fault)
                    .unwrap_or("SCOPE_EDGE"),
            );
        }
        if let Some(reason) = self.first_fault {
            Err(error(reason))
        } else {
            result
        }
    }
    fn facts(&self) -> EdgeFacts {
        let facts = self.lease.report();
        EdgeFacts {
            status: facts.workload_status,
            waits_known: facts.keeper_status.is_some()
                && (!facts.workload_spawned || facts.workload_status.is_some()),
            group_absent: facts.group_absent,
            terminal_eof: facts.terminal_eof,
            resource_proof: self.proof,
            io_required: facts.workload_spawned,
            io: self.pipes.facts(),
            first_fault: self
                .first_fault
                .or(facts.first_fault)
                .or(self.pipes.facts().first_fault),
        }
    }
    fn take_output(&mut self) -> CommandOutput {
        self.pipes.take_output()
    }
    fn take_events(&mut self) -> Vec<String> {
        let facts = self.lease.report();
        let parent = std::process::id();
        let nonce = self.lease.registration_nonce();
        let mut events = Vec::new();
        for (index, pid, status) in [
            (0, Some(facts.keeper_pid), None),
            (1, facts.workload_pid, None),
            (2, Some(facts.keeper_pid), facts.keeper_status),
            (3, facts.workload_pid, facts.workload_status),
        ] {
            let Some(pid) = pid else {
                continue;
            };
            if !self.published[index] && (index < 2 || status.is_some()) {
                self.published[index] = true;
                let kind = if index < 2 { "REGISTER" } else { "WAITED" };
                let value = status.map(ExitStatus::into_raw).unwrap_or(0);
                events.push(format!("{kind}|{parent}|{pid}|{nonce}|{value}"));
            }
        }
        events
    }
}

struct CooperativeEdge {
    child: Child,
    endpoint: Endpoint,
    nonce: String,
    status: Option<ExitStatus>,
    cancel_sent: Option<u64>,
    registered: bool,
    waited: bool,
    first_fault: Option<&'static str>,
    pipes: Option<PipeIo>,
}
impl Edge for CooperativeEdge {
    fn begin(&mut self, end: u64) -> io::Result<()> {
        if let Some(pipes) = &mut self.pipes {
            pipes.cancel_input();
        }
        if self.status.is_none()
            && !self.endpoint.is_settled()
            && self.cancel_sent.is_none_or(|previous| end < previous)
        {
            self.cancel_sent = Some(end);
            // V1's cancellation grammar requires a positive absolute end.
            // Zero is an already-expired local end, never a fresh allowance.
            if let Err(e) = self.endpoint.send_cancel_tighter(end.max(1)) {
                self.first_fault.get_or_insert(e.0);
                return Err(error(e.0));
            }
        }
        Ok(())
    }
    fn poll(&mut self) -> io::Result<()> {
        if let Some(pipes) = &mut self.pipes {
            pipes.poll();
            if let Some(reason) = pipes.facts().first_fault {
                self.first_fault.get_or_insert(reason);
            }
        }
        // Endpoint faults never skip the independently retained direct wait.
        if let Err(e) = self.endpoint.receive(false) {
            self.first_fault.get_or_insert(e.0);
        }
        if self.status.is_none() {
            match self.child.try_wait() {
                Ok(status) => self.status = status,
                Err(_) => {
                    self.first_fault.get_or_insert("SCOPE_CHILD_WAIT");
                }
            }
        }
        self.first_fault.map_or(Ok(()), |e| Err(error(e)))
    }
    fn facts(&self) -> EdgeFacts {
        EdgeFacts {
            status: self.status,
            waits_known: self.status.is_some(),
            terminal_eof: self.endpoint.is_settled(),
            resource_proof: self.status.is_some()
                && self.endpoint.is_settled()
                && self.first_fault.is_none(),
            first_fault: self.first_fault,
            io_required: self.pipes.is_some(),
            io: self.pipes.as_ref().map(PipeIo::facts).unwrap_or_default(),
            ..Default::default()
        }
    }
    fn take_output(&mut self) -> CommandOutput {
        self.pipes
            .as_mut()
            .map(PipeIo::take_output)
            .unwrap_or_else(|| CommandOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
    }
    fn take_events(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        let parent = std::process::id();
        let child = self.child.id();
        if !self.registered {
            self.registered = true;
            events.push(format!("REGISTER|{parent}|{child}|{}|0", self.nonce));
        }
        events.extend(self.endpoint.take_events());
        // A direct WAITED is published after all currently received descendant
        // records. Terminal+EOF remains a separate acceptance requirement.
        if let Some(status) = self
            .status
            .filter(|_| !self.waited && (self.endpoint.is_settled() || self.first_fault.is_some()))
        {
            self.waited = true;
            events.push(format!(
                "WAITED|{parent}|{child}|{}|{}",
                self.nonce,
                status.into_raw()
            ));
        }
        events
    }
}
enum ProductEdge {
    Group(GroupEdge),
    Cooperative(CooperativeEdge),
}
impl Edge for ProductEdge {
    fn progress_wait_sources(&self, descriptors: &mut Vec<libc::pollfd>) -> Option<u64> {
        match self {
            Self::Group(edge) => edge.progress_wait_sources(descriptors),
            Self::Cooperative(edge) => edge.progress_wait_sources(descriptors),
        }
    }
    fn progress_buffered_event(&self) -> bool {
        match self {
            Self::Group(edge) => edge.progress_buffered_event(),
            Self::Cooperative(edge) => edge.progress_buffered_event(),
        }
    }
    #[cfg(feature = "capture-profile")]
    fn repository_profile_stage(&self) -> Option<crate::capture_profile::Phase> {
        match self {
            Self::Group(edge) => edge.repository_profile_stage(),
            Self::Cooperative(edge) => edge.repository_profile_stage(),
        }
    }
    fn setup_buffered_event(&self) -> bool {
        match self {
            Self::Group(edge) => edge.setup_buffered_event(),
            Self::Cooperative(edge) => edge.setup_buffered_event(),
        }
    }
    fn post_join_pending(&self) -> bool {
        match self {
            Self::Group(edge) => edge.post_join_pending(),
            Self::Cooperative(edge) => edge.post_join_pending(),
        }
    }
    fn setup_wait_source(&self) -> Option<(i32, u64)> {
        match self {
            Self::Group(edge) => edge.setup_wait_source(),
            Self::Cooperative(edge) => edge.setup_wait_source(),
        }
    }
    #[cfg(feature = "dev-tools")]
    fn setup_trace(&self) -> Option<super::group::SetupTrace> {
        match self {
            Self::Group(edge) => edge.setup_trace(),
            Self::Cooperative(edge) => edge.setup_trace(),
        }
    }
    fn setup_pending(&self) -> bool {
        match self {
            Self::Group(edge) => edge.setup_pending(),
            Self::Cooperative(edge) => edge.setup_pending(),
        }
    }
    fn begin(&mut self, end: u64) -> io::Result<()> {
        match self {
            Self::Group(e) => e.begin(end),
            Self::Cooperative(e) => e.begin(end),
        }
    }
    fn poll(&mut self) -> io::Result<()> {
        match self {
            Self::Group(e) => e.poll(),
            Self::Cooperative(e) => e.poll(),
        }
    }
    fn facts(&self) -> EdgeFacts {
        match self {
            Self::Group(e) => e.facts(),
            Self::Cooperative(e) => e.facts(),
        }
    }
    fn take_output(&mut self) -> CommandOutput {
        match self {
            Self::Group(e) => e.take_output(),
            Self::Cooperative(e) => e.take_output(),
        }
    }
    fn take_events(&mut self) -> Vec<String> {
        match self {
            Self::Group(e) => e.take_events(),
            Self::Cooperative(e) => e.take_events(),
        }
    }
}
struct Slot<E> {
    edge: E,
    hard_end: u64,
    completion_end: Option<u64>,
    accepted_at: Option<u64>,
    output_taken: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum InstallerState {
    Preflight,
    Publishing,
}
struct InstallerPhase {
    end: u64,
    expected: usize,
    state: InstallerState,
}
struct Core<E: Edge> {
    owner: SharedOwner,
    identity: Rc<()>,
    slots: Vec<Slot<E>>,
    hard_end: u64,
    cancel_end: Option<u64>,
    io_reserved: usize,
    cursor: usize,
    finish_called: bool,
    finished: bool,
    installer: Option<InstallerPhase>,
    unfinished_end: u64,
    entry_selected: bool,
}
impl<E: Edge> Core<E> {
    fn new(owner: SharedOwner, hard_end: u64) -> Self {
        Self {
            owner,
            identity: Rc::new(()),
            slots: vec![],
            hard_end,
            cancel_end: None,
            io_reserved: 0,
            cursor: 0,
            finish_called: false,
            finished: false,
            installer: None,
            unfinished_end: hard_end,
            entry_selected: false,
        }
    }
    #[cfg(feature = "dev-tools")]
    fn first_fault_setup_trace(&self) -> Option<super::group::SetupTrace> {
        let first = self.owner.borrow().first_fault?;
        self.slots
            .iter()
            .filter_map(|slot| slot.edge.setup_trace())
            .find(|trace| {
                trace
                    .fault_origin
                    .is_some_and(|(reason, _)| reason == first.0)
            })
    }
    fn poison(&mut self, reason: &'static str) {
        self.owner.borrow_mut().poison(ProtocolError(reason));
    }
    fn admission_clock(&mut self, clock: &mut impl FnMut() -> io::Result<u64>) -> io::Result<u64> {
        match clock() {
            Ok(at) => Ok(at),
            Err(_) => {
                self.cancel(Some("SCOPE_CLOCK"), 0, 0);
                Err(error("SCOPE_CLOCK"))
            }
        }
    }
    fn admit(
        &mut self,
        at: u64,
        capacity: usize,
        create: impl FnOnce(SharedOwner) -> io::Result<E>,
    ) -> io::Result<SlotId> {
        if self.installer.is_some() {
            self.poison("SCOPE_INSTALLER_PHASE");
            return Err(error("SCOPE_INSTALLER_PHASE"));
        }
        self.admit_before(at, capacity, self.hard_end, |owner, _| create(owner))
    }
    fn admit_before(
        &mut self,
        at: u64,
        capacity: usize,
        requested_end: u64,
        create: impl FnOnce(SharedOwner, u64) -> io::Result<E>,
    ) -> io::Result<SlotId> {
        let hard_end = requested_end.min(self.hard_end);
        let admission = self.owner.borrow().check_admission();
        if let Err(reason) = admission {
            return Err(error(reason.0));
        }
        if self.finished || self.cancel_end.is_some() || at >= hard_end.saturating_sub(RESERVE_NS) {
            self.poison("SCOPE_ADMISSION");
            return Err(error("SCOPE_ADMISSION"));
        }
        let reserved = self
            .io_reserved
            .checked_add(capacity)
            .filter(|n| *n <= MAX_IO_BYTES);
        if self.slots.len() >= MAX_REGISTRATIONS
            || reserved.is_none()
            || self.slots.try_reserve(1).is_err()
        {
            self.poison("SCOPE_LIMIT");
            return Err(error("SCOPE_LIMIT"));
        }
        let edge = match create(self.owner.clone(), hard_end) {
            Ok(edge) => edge,
            Err(_) => {
                self.poison("SCOPE_SPAWN");
                return Err(error("SCOPE_SPAWN"));
            }
        };
        let id = SlotId {
            index: self.slots.len(),
            scope: self.identity.clone(),
        };
        // Capacity exists before factory spawn; the returned edge/handles are
        // retained before any subsequent publication or flags operation.
        self.slots.push(Slot {
            edge,
            hard_end,
            completion_end: None,
            accepted_at: None,
            output_taken: false,
        });
        self.io_reserved = reserved.unwrap();
        self.unfinished_end = self.unfinished_end.min(hard_end);
        Ok(id)
    }
    #[cfg(feature = "dev-tools")]
    fn admit_dev_client(
        &mut self,
        at: u64,
        capacity: usize,
        selected_end: u64,
        create: impl FnOnce(SharedOwner, u64) -> io::Result<E>,
    ) -> io::Result<SlotId> {
        if self.installer.is_some()
            || self.slots.len() >= 4
            || self.slots.iter().any(|slot| slot.accepted_at.is_none())
        {
            self.poison("DEV_CLIENT_ADMISSION");
            return Err(error("DEV_CLIENT_ADMISSION"));
        }
        let local = match at.checked_add(1_000_000_000) {
            Some(end) => end,
            None => {
                self.cancel(Some("SCOPE_CLOCK"), 0, 0);
                return Err(error("SCOPE_CLOCK"));
            }
        };
        self.admit_before(at, capacity, selected_end.min(local), create)
    }
    #[cfg(feature = "dev-tools")]
    fn release_dev_completed(&mut self, ledgers_complete: bool) -> io::Result<()> {
        if !ledgers_complete
            || self.installer.is_some()
            || self.slots.is_empty()
            || self.cancel_end.is_some()
            || self.finish_called
            || self.owner.borrow().check_admission().is_err()
            || self.slots.iter().any(|slot| {
                !slot.output_taken
                    || !slot.edge.facts().resources_complete()
                    || slot.accepted_at.is_none_or(|at| at >= slot.hard_end)
            })
        {
            self.poison("DEV_CLIENT_COLLECTION");
            return Err(error("DEV_CLIENT_COLLECTION"));
        }
        // Only the cache of UNFINISHED ends is released. Original owner/slot
        // ends and all immutable acceptance facts remain unchanged.
        self.unfinished_end = self.hard_end;
        Ok(())
    }
    fn begin_installer(&mut self, at: u64, expected: usize) -> io::Result<()> {
        if self.installer.is_some()
            || !self.slots.is_empty()
            || !(1..=2).contains(&expected)
            || self.finished
            || self.finish_called
            || self.cancel_end.is_some()
            || self.owner.borrow().check_admission().is_err()
        {
            self.cancel(Some("SCOPE_INSTALLER_PHASE"), at, self.hard_end);
            return Err(error("SCOPE_INSTALLER_PHASE"));
        }
        let Some(operation_end) = at.checked_add(INSTALL_OPERATION_NS) else {
            self.cancel(Some("SCOPE_CLOCK"), 0, 0);
            return Err(error("SCOPE_CLOCK"));
        };
        // Library callers may supply a more generous containing owner. This
        // operation can only restrict it; main's earlier entry bound still wins.
        self.hard_end = self.hard_end.min(operation_end);
        self.unfinished_end = self.unfinished_end.min(self.hard_end);
        let Some(local_end) = at.checked_add(ORDINARY_OPERATION_NS) else {
            self.cancel(Some("SCOPE_CLOCK"), 0, 0);
            return Err(error("SCOPE_CLOCK"));
        };
        let end = self.hard_end.min(local_end);
        self.installer = Some(InstallerPhase {
            end,
            expected,
            state: InstallerState::Preflight,
        });
        if at >= end.saturating_sub(RESERVE_NS) {
            self.cancel(Some("SCOPE_ADMISSION"), at, end);
            return Err(error("SCOPE_ADMISSION"));
        }
        Ok(())
    }
    fn installer_end(&self) -> io::Result<u64> {
        self.installer
            .as_ref()
            .filter(|phase| {
                phase.state == InstallerState::Preflight && self.slots.len() < phase.expected
            })
            .map(|phase| phase.end)
            .ok_or_else(|| error("SCOPE_INSTALLER_PHASE"))
    }
    fn admit_installer(
        &mut self,
        at: u64,
        capacity: usize,
        create: impl FnOnce(SharedOwner, u64) -> io::Result<E>,
    ) -> io::Result<SlotId> {
        let end = match self.installer_end() {
            Ok(end) => end,
            Err(error) => {
                self.cancel(Some("SCOPE_INSTALLER_PHASE"), at, self.hard_end);
                return Err(error);
            }
        };
        self.admit_before(at, capacity, end, create)
    }
    fn refine_entry(&mut self, end: u64, at: u64) -> io::Result<()> {
        self.hard_end = self.hard_end.min(end);
        if self.entry_selected
            || !self.slots.is_empty()
            || self.installer.is_some()
            || self.finished
            || self.cancel_end.is_some()
            || self.owner.borrow().check_admission().is_err()
            || at >= self.hard_end.saturating_sub(RESERVE_NS)
        {
            self.cancel(Some("SCOPE_ENTRY_SELECTION"), at, self.hard_end);
            return Err(error("SCOPE_ENTRY_SELECTION"));
        }
        self.entry_selected = true;
        Ok(())
    }
    fn active_end(&self) -> u64 {
        let mut end = self.hard_end;
        if let Some(phase) = &self.installer {
            if phase.state == InstallerState::Preflight {
                end = end.min(phase.end);
            }
        }
        end.min(self.unfinished_end)
    }
    fn seal_installer(
        &mut self,
        ledgers_complete: bool,
        clock: &mut impl FnMut() -> io::Result<u64>,
    ) -> io::Result<()> {
        let valid = self.installer.as_ref().is_some_and(|phase| {
            phase.state == InstallerState::Preflight
                && self.slots.len() == phase.expected
                && self.slots.iter().all(|slot| {
                    let facts = slot.edge.facts();
                    slot.output_taken
                        && slot
                            .accepted_at
                            .is_some_and(|at| at < slot.hard_end && at < phase.end)
                        && slot.hard_end <= phase.end
                        && facts.resources_complete()
                        && facts.status.is_some()
                        && facts.group_absent
                        && facts.terminal_eof
                        && facts.first_fault.is_none()
                        && facts.io.first_fault.is_none()
                })
        }) && ledgers_complete
            && !self.finished
            && !self.finish_called
            && self.cancel_end.is_none()
            && self.owner.borrow().check_admission().is_ok();
        let at = self.admission_clock(clock)?;
        let end = self.active_end();
        if !valid || at >= end || at >= self.hard_end.saturating_sub(RESERVE_NS) {
            self.cancel(Some("SCOPE_PREFLIGHT"), at, end);
            return Err(error("SCOPE_PREFLIGHT"));
        }
        self.installer.as_mut().unwrap().state = InstallerState::Publishing;
        // No unaccepted cohort slot remains. Retain every immutable slot end;
        // only its now-inactive timer stops restricting the parent operation.
        self.unfinished_end = self.hard_end;
        Ok(())
    }
    fn publication_live(
        &mut self,
        ledgers_complete: bool,
        clock: &mut impl FnMut() -> io::Result<u64>,
    ) -> io::Result<()> {
        let valid = self
            .installer
            .as_ref()
            .is_some_and(|phase| phase.state == InstallerState::Publishing)
            && ledgers_complete
            && self.all_resources()
            && !self.finished
            && !self.finish_called
            && self.cancel_end.is_none()
            && self.owner.borrow().check_admission().is_ok();
        let at = self.admission_clock(clock)?;
        if !valid || at >= self.hard_end.saturating_sub(RESERVE_NS) {
            self.cancel(Some("SCOPE_PUBLICATION"), at, self.hard_end);
            return Err(error("SCOPE_PUBLICATION"));
        }
        Ok(())
    }
    fn terminal_fresh(&mut self, clock: &mut impl FnMut() -> io::Result<u64>) -> bool {
        let at = match self.admission_clock(clock) {
            Ok(at) => at,
            Err(_) => return false,
        };
        let end = self.hard_end.min(self.cancel_end.unwrap_or(self.hard_end));
        if at >= end {
            self.cancel(Some("SCOPE_TERMINAL_DEADLINE"), at, end);
            return false;
        }
        self.finished
            && self.finish_called
            && self.all_resources()
            && self.owner.borrow().first_fault.is_none()
            && self
                .installer
                .as_ref()
                .is_some_and(|phase| phase.state == InstallerState::Publishing)
    }
    fn index(&mut self, id: &SlotId) -> io::Result<usize> {
        if !Rc::ptr_eq(&id.scope, &self.identity) || id.index >= self.slots.len() {
            self.poison("SCOPE_SLOT_ID");
            return Err(error("SCOPE_SLOT_ID"));
        }
        Ok(id.index)
    }
    fn request_complete(&mut self, id: &SlotId, at: u64) -> io::Result<()> {
        let index = self.index(id)?;
        if self.slots[index].completion_end.is_some() {
            return Ok(());
        }
        // Completing this slot must leave its containing owner open. Global
        // cancellation/final finish use cancel(), never this local transition.
        let end = self
            .hard_end
            .min(self.slots[index].hard_end)
            .min(at.saturating_add(RESERVE_NS));
        self.slots[index].completion_end = Some(end);
        let result = self.slots[index].edge.begin(end);
        if result.is_err() {
            self.poison(
                self.slots[index]
                    .edge
                    .facts()
                    .first_fault
                    .unwrap_or("SCOPE_BEGIN"),
            );
        }
        result
    }
    fn cancel(&mut self, reason: Option<&'static str>, at: u64, requested: u64) {
        if let Some(reason) = reason {
            self.poison(reason);
        }
        self.owner.borrow_mut().admission_closed = true;
        let end = self
            .cancel_end
            .unwrap_or(u64::MAX)
            .min(self.hard_end)
            .min(self.active_end())
            .min(requested)
            .min(at.saturating_add(RESERVE_NS));
        if self.cancel_end == Some(end) {
            return;
        }
        self.cancel_end = Some(end);
        // Broadcast ALL intents before any poll/wait, even when begin fails.
        for index in 0..self.slots.len() {
            let slot = &mut self.slots[index];
            if slot.accepted_at.is_some() {
                continue;
            }
            slot.completion_end = Some(slot.completion_end.unwrap_or(u64::MAX).min(end));
            if slot.edge.begin(slot.completion_end.unwrap()).is_err() {
                let reason = slot.edge.facts().first_fault.unwrap_or("SCOPE_BEGIN");
                self.poison(reason);
            }
        }
    }
    fn poll(&mut self, clock: &mut impl FnMut() -> io::Result<u64>) {
        let at = match clock() {
            Ok(at) => at,
            Err(_) => {
                self.cancel(Some("SCOPE_CLOCK"), 0, 0);
                0
            }
        };
        let fault = self.owner.borrow().first_fault;
        if fault.is_some() && self.cancel_end.is_none() {
            self.cancel(None, at, self.hard_end);
        }
        if at >= self.active_end().saturating_sub(RESERVE_NS) && self.cancel_end.is_none() {
            self.cancel(Some("SCOPE_OPERATION_DEADLINE"), at, self.active_end());
        }
        let len = self.slots.len();
        for _ in 0..len.min(SLOTS_PER_TURN) {
            let index = self.cursor % len;
            self.cursor = (index + 1) % len;
            self.poll_slot(index, clock);
        }
    }
    fn poll_slot(&mut self, index: usize, clock: &mut impl FnMut() -> io::Result<u64>) {
        if self.slots[index].accepted_at.is_some() {
            return;
        }
        if self.slots[index].edge.poll().is_err() {
            self.poison(
                self.slots[index]
                    .edge
                    .facts()
                    .first_fault
                    .unwrap_or("SCOPE_POLL"),
            );
            match clock() {
                Ok(at) => self.cancel(None, at, self.hard_end),
                Err(_) => self.cancel(Some("SCOPE_CLOCK"), 0, 0),
            }
        }
        let facts = self.slots[index].edge.facts();
        if facts.status.is_some() && self.slots[index].completion_end.is_none() {
            match clock() {
                Err(_) => self.cancel(Some("SCOPE_CLOCK"), 0, 0),
                Ok(at) => {
                    let id = SlotId {
                        index,
                        scope: self.identity.clone(),
                    };
                    if self.request_complete(&id, at).is_err() {
                        self.cancel(None, at, self.hard_end);
                    }
                }
            }
        }
        if facts.resources_complete() {
            let end = self.slots[index]
                .completion_end
                .unwrap_or(0)
                .min(self.slots[index].hard_end)
                .min(self.hard_end);
            match clock() {
                Ok(at) if at < end => self.slots[index].accepted_at = Some(at),
                _ => {
                    self.poison("SCOPE_PROOF_DEADLINE");
                    self.cancel(None, 0, 0);
                }
            }
        }
    }
    fn poll_post_joined(
        &mut self,
        start: usize,
        clock: &mut impl FnMut() -> io::Result<u64>,
    ) -> bool {
        let mut observed = false;
        // Only the edges visited by the containing fair turn can be newly
        // joined. No recursion, new child, cursor reset or second setup loop.
        let len = self.slots.len();
        for offset in 0..len.min(SLOTS_PER_TURN) {
            let index = (start + offset) % len;
            if self.slots[index].accepted_at.is_some()
                || !self.slots[index].edge.post_join_pending()
            {
                continue;
            }
            if self.finished || self.cancel_end.is_some() {
                return observed;
            }
            let at = match self.admission_clock(clock) {
                Ok(at) => at,
                Err(_) => return observed,
            };
            let admission = self.owner.borrow().check_admission();
            if let Err(reason) = admission {
                self.cancel(Some(reason.0), at, self.hard_end);
                return observed;
            }
            if at >= self.active_end().saturating_sub(RESERVE_NS) {
                self.cancel(Some("SCOPE_OPERATION_DEADLINE"), at, self.active_end());
                return observed;
            }
            // GroupEdge's existing Stage::Park path clears its flag and uses
            // the original poll_parked parser and before/after end checks.
            observed = true;
            self.poll_slot(index, clock);
        }
        observed
    }
    fn all_resources(&self) -> bool {
        self.slots.iter().all(|s| s.accepted_at.is_some())
    }
    fn finish_at(&mut self, at: u64) -> bool {
        if self.finished {
            return true;
        }
        if at >= self.cancel_end.unwrap_or(0) {
            self.poison("SCOPE_CLEANUP_DEADLINE");
            self.finished = true;
        } else if self.all_resources() {
            self.finished = true;
        }
        self.finished
    }
    // The public finish entrypoint and finite regressions use this SAME
    // envelope. Clock injection does not replace any ownership transitions.
    #[cfg(test)]
    fn finish_with(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<u64>,
        pause: &mut impl FnMut(),
    ) {
        self.finish_with_progress(clock, pause, &mut |_| {});
    }
    fn finish_with_progress(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<u64>,
        pause: &mut impl FnMut(),
        progress: &mut impl FnMut(&mut Self),
    ) {
        self.finish_called = true;
        if self.finished {
            return;
        }
        match clock() {
            Ok(at) => self.cancel(None, at, self.hard_end),
            Err(_) => self.cancel(Some("SCOPE_CLOCK"), 0, 0),
        }
        loop {
            progress(self);
            self.poll(clock);
            progress(self);
            let at = match clock() {
                Ok(at) => at,
                Err(_) => {
                    // Broadcast the irreversible zero end BEFORE marking
                    // finished, including the final fresh-clock placement.
                    self.cancel(Some("SCOPE_CLOCK"), 0, 0);
                    0
                }
            };
            if self.finish_at(at) {
                break;
            }
            #[cfg(feature = "capture-profile")]
            {
                let _span = crate::capture_profile::Span::wait(
                    crate::capture_profile::Phase::RepoFinishWait,
                    1_000_000,
                );
                pause();
            }
            #[cfg(not(feature = "capture-profile"))]
            pause();
        }
        self.finished = true;
    }
    // No sibling preparation/spawn while a prior short READY/PARK handshake
    // still requires this same entry thread. Running/cleanup may overlap.
    fn repository_admission_ready(&mut self) -> io::Result<bool> {
        let admission = self.owner.borrow().check_admission();
        if let Err(reason) = admission {
            return Err(error(reason.0));
        }
        if self.finished || self.cancel_end.is_some() {
            self.poison("SCOPE_ADMISSION");
            return Err(error("SCOPE_ADMISSION"));
        }
        Ok(!self
            .slots
            .iter()
            .any(|slot| slot.accepted_at.is_none() && slot.edge.setup_pending()))
    }
    fn wait_repository_setup(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<u64>,
        poll: &mut impl FnMut(&mut [libc::pollfd], i32) -> io::Result<i32>,
    ) -> io::Result<()> {
        self.repository_admission_ready()?;
        let sources: Vec<_> = self
            .slots
            .iter()
            .filter(|slot| slot.accepted_at.is_none() && slot.edge.setup_pending())
            .filter_map(|slot| slot.edge.setup_wait_source())
            .collect();
        if sources.is_empty() || sources.iter().any(|(fd, _)| *fd < 0) {
            self.poison("SCOPE_SETUP_WAIT");
            return Err(error("SCOPE_SETUP_WAIT"));
        }
        let end = sources
            .iter()
            .map(|(_, end)| *end)
            .min()
            .unwrap()
            .min(self.active_end().saturating_sub(RESERVE_NS));
        let mut descriptors: Vec<_> = sources
            .iter()
            .map(|(fd, _)| libc::pollfd {
                fd: *fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        let at = self.admission_clock(clock)?;
        if at >= end {
            return Ok(());
        }
        if self.slots.iter().any(|slot| {
            slot.accepted_at.is_none()
                && slot.edge.setup_pending()
                && slot.edge.setup_buffered_event()
        }) {
            return Ok(());
        }
        // Block only while at least a full millisecond remains. Readable
        // protocol traffic wakes immediately; the original last submillisecond
        // remains nonblocking, and no absolute end is reset or extended.
        let timeout = ((end - at) / 1_000_000).min(1) as i32;
        #[cfg(feature = "capture-profile")]
        let outcome = {
            let _span = crate::capture_profile::Span::wait(
                crate::capture_profile::Phase::RepoSetupWait,
                timeout as u64 * 1_000_000,
            );
            poll(&mut descriptors, timeout)
        };
        #[cfg(not(feature = "capture-profile"))]
        let outcome = poll(&mut descriptors, timeout);
        match outcome {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(()),
            Ok(count)
                if count >= 0
                    && descriptors
                        .iter()
                        .all(|fd| fd.revents & libc::POLLNVAL == 0) =>
            {
                Ok(())
            }
            _ => {
                self.poison("SCOPE_SETUP_WAIT");
                Err(error("SCOPE_SETUP_WAIT"))
            }
        }
        // Readiness/timeout/HUP/EINTR never advance a protocol state. The next
        // original progress turn parses frames and checks deadlines/signals.
    }
    #[cfg(feature = "capture-profile")]
    fn repository_wait_phase(&self) -> crate::capture_profile::Phase {
        use crate::capture_profile::Phase;
        let mut mask = 0u8;
        for slot in self.slots.iter().filter(|slot| slot.accepted_at.is_none()) {
            mask |= match slot.edge.repository_profile_stage() {
                Some(Phase::RepoRunningPoll) => 1,
                // Done can retain physical pipe EOF collection. Keep it in the
                // cleanup wait category; its poll time has a distinct counter.
                Some(Phase::RepoCleaningPoll | Phase::RepoDonePoll) => 2,
                Some(Phase::RepoReadyPoll | Phase::RepoParkPoll) => 4,
                _ => 8,
            };
        }
        match mask {
            1 => Phase::RepoRunningWait,
            2 => Phase::RepoCleaningWait,
            0 | 8 => Phase::RepoUnknownWait,
            _ => Phase::RepoMixedWait,
        }
    }
    fn progress_wait_clock(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<u64>,
        end: u64,
    ) -> io::Result<u64> {
        let at = self.admission_clock(clock)?;
        if super::signals::pending() {
            self.cancel(Some("SCOPE_INTERRUPTED"), at, self.hard_end);
            return Err(error("SCOPE_INTERRUPTED"));
        }
        self.repository_admission_ready()?;
        if at >= self.active_end().saturating_sub(RESERVE_NS) {
            self.cancel(Some("SCOPE_OPERATION_DEADLINE"), at, self.active_end());
            return Err(error("SCOPE_OPERATION_DEADLINE"));
        }
        if at >= end {
            // The local end caps waiting only. Preserve every sibling
            // cleanup allowance in the original scope cancellation envelope.
            self.cancel(Some("SCOPE_PROOF_DEADLINE"), at, self.hard_end);
            return Err(error("SCOPE_PROOF_DEADLINE"));
        }
        Ok(at)
    }
    fn wait_running_progress(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<u64>,
        poll: &mut impl FnMut(&mut [libc::pollfd], i32) -> io::Result<i32>,
        pause: &mut impl FnMut(),
    ) -> io::Result<()> {
        let mut end = self.active_end().saturating_sub(RESERVE_NS);
        let mut descriptors = Vec::new();
        if descriptors
            .try_reserve(self.slots.len().min(MAX_REGISTRATIONS) * 4)
            .is_err()
        {
            self.poison("SCOPE_LIMIT");
            return Err(error("SCOPE_LIMIT"));
        }
        let mut buffered = false;
        for slot in self.slots.iter().filter(|slot| slot.accepted_at.is_none()) {
            end = end
                .min(slot.hard_end)
                .min(slot.completion_end.unwrap_or(u64::MAX));
            if let Some(edge_end) = slot.edge.progress_wait_sources(&mut descriptors) {
                end = end.min(edge_end);
            }
            buffered |= slot.edge.progress_buffered_event();
        }
        if descriptors.iter().any(|fd| fd.fd < 0) {
            self.poison("SCOPE_POLL");
            return Err(error("SCOPE_POLL"));
        }
        let at = self.progress_wait_clock(clock, end)?;
        if buffered {
            return Ok(());
        }
        // Same original1ms maximum and original earliest absolute bound. The
        // final fraction remains nonblocking; never round it up or reset it.
        let timeout = ((end - at) / 1_000_000).min(1) as i32;
        #[cfg(feature = "capture-profile")]
        let phase = self.repository_wait_phase();
        if descriptors.is_empty() {
            // EOF/HUP already consumed: sockets cannot wake a Child::try_wait.
            // Retain the old short polling cadence rather than spin on HUP.
            if timeout == 0 {
                return Ok(());
            }
            #[cfg(feature = "capture-profile")]
            let _span = crate::capture_profile::Span::wait(phase, 1_000_000);
            pause();
        } else {
            #[cfg(feature = "capture-profile")]
            let outcome = {
                let _span = crate::capture_profile::Span::wait(phase, timeout as u64 * 1_000_000);
                poll(&mut descriptors, timeout)
            };
            #[cfg(not(feature = "capture-profile"))]
            let outcome = poll(&mut descriptors, timeout);
            match outcome {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Ok(count)
                    if count >= 0
                        && descriptors
                            .iter()
                            .all(|fd| fd.revents & libc::POLLNVAL == 0) => {}
                _ => {
                    self.poison("SCOPE_POLL");
                    return Err(error("SCOPE_POLL"));
                }
            }
        }
        self.progress_wait_clock(clock, end)?;
        // Readiness/timeout/HUP/EINTR only schedule the original next fair
        // turn (including upstream CANCEL pumping); NONE is resource proof.
        Ok(())
    }
    fn wait_command_progress(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<u64>,
        poll: &mut impl FnMut(&mut [libc::pollfd], i32) -> io::Result<i32>,
        pause: &mut impl FnMut(),
    ) -> io::Result<()> {
        // The final admitted edge still owns its original short READY/PARK
        // window. Do not return to a blind timer while its socket can wake us.
        if self.repository_admission_ready()? {
            self.wait_running_progress(clock, poll, pause)
        } else {
            self.wait_repository_setup(clock, poll)
        }
    }
    // Validate the entire fixed cohort BEFORE consuming any result. An earlier
    // accepted sibling cannot leak output while another is pending or invalid.
    fn take_repository_results(
        &mut self,
        slots: &[SlotId; 2],
    ) -> io::Result<Option<[CommandResult; 2]>> {
        let fault = self.owner.borrow().first_fault;
        if let Some(reason) = fault {
            return Err(error(reason.0));
        }
        let mut pending = false;
        for (position, id) in slots.iter().enumerate() {
            let index = self.index(id)?;
            if slots[..position].iter().any(|other| other.index == index) {
                self.poison("SCOPE_RESULT");
                return Err(error("SCOPE_RESULT"));
            }
            let slot = &self.slots[index];
            if slot.output_taken {
                self.poison("SCOPE_RESULT");
                return Err(error("SCOPE_RESULT"));
            }
            if slot.accepted_at.is_none() {
                pending = true;
            } else {
                let facts = slot.edge.facts();
                if !facts.resources_complete()
                    || !facts.group_absent
                    || !facts.terminal_eof
                    || facts.status.is_none()
                    || facts.first_fault.is_some()
                    || facts.io.first_fault.is_some()
                {
                    self.poison("SCOPE_RESULT");
                    return Err(error("SCOPE_RESULT"));
                }
            }
        }
        if pending {
            return Ok(None);
        }
        Ok(Some([
            self.take_result(&slots[0])?,
            self.take_result(&slots[1])?,
        ]))
    }
    fn take_result(&mut self, id: &SlotId) -> io::Result<CommandResult> {
        let index = self.index(id)?;
        let fault = self.owner.borrow().first_fault;
        if let Some(reason) = fault {
            return Err(error(reason.0));
        }
        let slot = &self.slots[index];
        let status = slot.edge.facts().status;
        if slot.accepted_at.is_none() || slot.output_taken || status.is_none() {
            self.poison("SCOPE_RESULT");
            return Err(error("SCOPE_RESULT"));
        }
        let slot = &mut self.slots[index];
        slot.output_taken = true;
        Ok(CommandResult {
            status: status.unwrap(),
            output: slot.edge.take_output(),
        })
    }
}

fn upstream_park_probe_allowed(work: Option<&super::protocol::PollWork>) -> bool {
    work.is_none_or(|work| {
        work.reads < super::protocol::MAX_READS && work.parsed < super::protocol::MAX_PARSE
    })
}

fn wait_with_upstream_budget(
    work: Option<&super::protocol::PollWork>,
    wait: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    if !upstream_park_probe_allowed(work) {
        return Ok(());
    }
    wait()
}

fn poll_joined_after_drain<E: Edge>(
    core: &mut Core<E>,
    start: usize,
    clock: &mut impl FnMut() -> io::Result<u64>,
    drain: &mut impl FnMut(&mut Core<E>) -> bool,
) -> bool {
    if !drain(core) {
        return false;
    }
    core.poll_post_joined(start, clock)
}

fn await_repository_admission<E: Edge>(
    core: &mut Core<E>,
    progress: &mut impl FnMut(&mut Core<E>),
    pause: &mut impl FnMut(&mut Core<E>) -> io::Result<()>,
) -> io::Result<()> {
    loop {
        progress(core);
        if core.repository_admission_ready()? {
            return Ok(());
        }
        pause(core)?;
    }
}

// Queue only inside the original containing end. Actual keeper preparation
// still begins at GroupLease::spawn's unchanged timestamp and setup policy.
#[cfg(test)]
fn await_hook_setup_capacity<E: Edge>(
    core: &mut Core<E>,
    progress: &mut impl FnMut(&mut Core<E>),
    clock: &mut impl FnMut() -> io::Result<u64>,
    attempt: &mut impl FnMut() -> io::Result<bool>,
    pause: &mut impl FnMut(),
) -> io::Result<()> {
    await_hook_setup_capacity_until(core, progress, clock, attempt, pause, None)
}

fn await_hook_setup_capacity_until<E: Edge>(
    core: &mut Core<E>,
    progress: &mut impl FnMut(&mut Core<E>),
    clock: &mut impl FnMut() -> io::Result<u64>,
    attempt: &mut impl FnMut() -> io::Result<bool>,
    pause: &mut impl FnMut(),
    admission_ceiling: Option<u64>,
) -> io::Result<()> {
    loop {
        progress(core);
        let at = core.admission_clock(clock)?;
        let ready = core.repository_admission_ready()?;
        let admission_end = core.active_end().min(admission_ceiling.unwrap_or(u64::MAX));
        if at >= admission_end.saturating_sub(RESERVE_NS) {
            core.cancel(Some("SCOPE_SETUP_DEADLINE"), at, core.active_end());
            return Err(error("SCOPE_SETUP_DEADLINE"));
        }
        if ready
            && attempt().map_err(|_| {
                core.poison("SCOPE_SETUP_GATE");
                error("SCOPE_SETUP_GATE")
            })?
        {
            // Parent cancellation, clock failure or shortening during flock
            // cannot grant a stale admission after the descriptor was acquired.
            progress(core);
            let fresh = core.admission_clock(clock)?;
            let ready = core.repository_admission_ready()?;
            let admission_end = core.active_end().min(admission_ceiling.unwrap_or(u64::MAX));
            if !ready || fresh >= admission_end.saturating_sub(RESERVE_NS) {
                core.cancel(Some("SCOPE_SETUP_DEADLINE"), fresh, core.active_end());
                return Err(error("SCOPE_SETUP_DEADLINE"));
            }
            return Ok(());
        }
        pause();
    }
}

#[derive(Clone, Debug)]
pub struct SlotReport {
    pub slot: usize,
    pub status: Option<ExitStatus>,
    pub waits_known: bool,
    pub group_absent: bool,
    pub terminal_eof: bool,
    pub io_required: bool,
    pub io: IoFacts,
    pub accepted_at: Option<u64>,
    pub completion_end: Option<u64>,
    pub first_fault: Option<&'static str>,
    #[cfg(feature = "dev-tools")]
    pub setup_trace: Option<super::group::SetupTrace>,
}
#[derive(Clone, Debug)]
pub struct ScopeReport {
    pub first_fault: Option<ProtocolError>,
    pub admission_closed: bool,
    pub finish_called: bool,
    pub all_resources: bool,
    pub clean: bool,
    pub slots: Vec<SlotReport>,
}
pub struct CommandResult {
    pub status: ExitStatus,
    pub output: CommandOutput,
}
pub struct Scope {
    core: Core<ProductEdge>,
    upstream: Option<Endpoint>,
    ledger: BTreeMap<Registration, bool>,
    terminal_sent: bool,
    hook_setup_directory: Option<File>,
    _signals: Option<super::signals::InstalledHandlers>,
}

impl Scope {
    /// Called on the entry thread after image bootstrap and before CLI parsing
    /// or runtime construction. Absence and malformed metadata are distinct.
    pub fn entry_from_environment(local_ns: u64) -> io::Result<Option<Self>> {
        if !super::attachment::present() {
            return Ok(None);
        }
        let at = now()?;
        let end = at
            .checked_add(local_ns)
            .ok_or_else(|| error("SCOPE_CLOCK"))?;
        Self::attach_at(at, end)
    }
    pub(crate) fn attach_at(at: u64, end: u64) -> io::Result<Option<Self>> {
        let Some((attachment, stream)) = super::attachment::consume(at, end)? else {
            return Ok(None);
        };
        let signals = super::signals::install()?;
        let owner: SharedOwner = Default::default();
        let mut endpoint = Endpoint::with_owner(
            stream,
            attachment.nonce,
            Some(attachment.parent),
            owner.clone(),
        )
        .map_err(|e| error(e.0))?;
        endpoint.send_live().map_err(|e| error(e.0))?;
        Ok(Some(Self {
            core: Core::new(owner, attachment.end),
            upstream: Some(endpoint),
            ledger: BTreeMap::new(),
            terminal_sent: false,
            hook_setup_directory: None,
            _signals: Some(signals),
        }))
    }
    pub(crate) fn refine_entry(&mut self, end: u64) -> io::Result<()> {
        self.poll();
        let at = self.core.admission_clock(&mut now)?;
        self.core.refine_entry(end, at)?;
        self.poll();
        if self.failed() {
            Err(error("SCOPE_ENTRY_SELECTION"))
        } else {
            Ok(())
        }
    }
    pub fn local_entry(hard_end: u64) -> io::Result<Self> {
        let signals = super::signals::install()?;
        let mut scope = Self::with_owner(Default::default(), hard_end);
        scope._signals = Some(signals);
        Ok(scope)
    }
    pub fn hard_end(&self) -> u64 {
        self.core.hard_end
    }
    pub fn slot_status(&self, id: &SlotId) -> Option<ExitStatus> {
        if !Rc::ptr_eq(&id.scope, &self.core.identity) {
            return None;
        }
        self.core
            .slots
            .get(id.index)
            .and_then(|slot| slot.edge.facts().status)
    }
    pub fn slot_complete(&self, id: &SlotId) -> bool {
        Rc::ptr_eq(&id.scope, &self.core.identity)
            && self
                .core
                .slots
                .get(id.index)
                .is_some_and(|slot| slot.accepted_at.is_some())
            && self.core.owner.borrow().first_fault.is_none()
    }
    pub fn failed(&self) -> bool {
        self.core.owner.borrow().first_fault.is_some()
    }
    pub(crate) fn tighten_end(&mut self, end: u64) {
        self.core.hard_end = self.core.hard_end.min(end);
    }
    pub(crate) fn owner_state(&self) -> SharedOwner {
        self.core.owner.clone()
    }
    pub(crate) fn set_hook_setup_directory(&mut self, directory: File) -> io::Result<()> {
        if self.hook_setup_directory.is_some() || !self.core.slots.is_empty() || self.core.finished
        {
            return Err(error("SCOPE_SETUP_GATE"));
        }
        self.core
            .owner
            .borrow()
            .check_admission()
            .map_err(|e| error(e.0))?;
        self.hook_setup_directory = Some(directory);
        Ok(())
    }
    fn await_hook_setup_admission(&mut self) -> io::Result<Option<HookSetupAdmission>> {
        let Some(directory) = self.hook_setup_directory.as_ref() else {
            return Ok(None);
        };
        // Duplicating the directory descriptor does not duplicate a lock.
        let directory = directory
            .try_clone()
            .map_err(|_| error("SCOPE_SETUP_GATE"))?;
        self.await_hook_capacity(directory, HookCapacityKind::Leaf, None)
            .map(Some)
    }
    #[allow(dead_code)] // Called by the standalone guard, not ordinary product scopes.
    pub(crate) fn await_hook_core_admission(
        &mut self,
        directory: &File,
        leaf_end: u64,
    ) -> io::Result<HookCoreAdmission> {
        if !self.core.slots.is_empty() || self.core.finished || self.hook_setup_directory.is_some()
        {
            return Err(error("SCOPE_SETUP_GATE"));
        }
        let directory = directory
            .try_clone()
            .map_err(|_| error("SCOPE_SETUP_GATE"))?;
        self.await_hook_capacity(directory, HookCapacityKind::Core, Some(leaf_end))
            .map(|permit| HookCoreAdmission { _permit: permit })
    }
    fn await_hook_capacity(
        &mut self,
        directory: File,
        kind: HookCapacityKind,
        admission_ceiling: Option<u64>,
    ) -> io::Result<HookSetupAdmission> {
        let mut candidates = (0..HOOK_SETUP_SLOTS)
            .map(|slot| HookSetupAdmission::open_kind(&directory, slot, kind).map(Some))
            .collect::<io::Result<Vec<_>>>()?;
        let mut selected = None;
        let upstream = &mut self.upstream;
        let ledger = &mut self.ledger;
        await_hook_setup_capacity_until(
            &mut self.core,
            &mut |core| Self::poll_turn(core, upstream, ledger),
            &mut now,
            &mut || {
                for candidate in &mut candidates {
                    if candidate.as_mut().unwrap().try_lock(&directory)? {
                        selected = candidate.take();
                        return Ok(true);
                    }
                }
                Ok(false)
            },
            &mut || std::thread::sleep(std::time::Duration::from_millis(1)),
            admission_ceiling,
        )?;
        selected.ok_or_else(|| error("SCOPE_SETUP_GATE"))
    }
    pub(crate) fn first_fault(&self) -> Option<ProtocolError> {
        self.core.owner.borrow().first_fault
    }
    #[cfg(feature = "dev-tools")]
    pub(crate) fn first_fault_setup_trace(&self) -> Option<super::group::SetupTrace> {
        self.core.first_fault_setup_trace()
    }
    pub(crate) fn begin_installer_preflight(&mut self, expected: usize) -> io::Result<()> {
        self.poll();
        let at = self.core.admission_clock(&mut now)?;
        self.core.begin_installer(at, expected)
    }
    fn ledgers_complete(&self) -> bool {
        self.ledger.values().all(|waited| *waited)
            && self
                .upstream
                .as_ref()
                .is_none_or(Endpoint::sent_resources_complete)
    }
    pub(crate) fn seal_installer_preflight(&mut self) -> io::Result<()> {
        self.poll();
        if self.terminal_sent {
            self.begin_cancel(ProtocolError("SCOPE_PREFLIGHT"), 0);
        }
        // A selected cohort has at most two slots; one fair turn visits all.
        let complete = self.ledgers_complete();
        self.core.seal_installer(complete, &mut now)
    }
    pub(crate) fn check_installer_publication(&mut self) -> io::Result<()> {
        self.poll();
        if self.terminal_sent {
            self.begin_cancel(ProtocolError("SCOPE_PUBLICATION"), 0);
        }
        let complete = self.ledgers_complete();
        self.core.publication_live(complete, &mut now)
    }
    pub fn with_owner(owner: SharedOwner, hard_end: u64) -> Self {
        Self {
            core: Core::new(owner, hard_end),
            upstream: None,
            ledger: BTreeMap::new(),
            terminal_sent: false,
            hook_setup_directory: None,
            _signals: None,
        }
    }
    fn pump<E: Edge>(
        core: &mut Core<E>,
        upstream: &mut Option<Endpoint>,
        ledger: &mut BTreeMap<Registration, bool>,
        turn_start: usize,
    ) {
        if super::signals::pending() {
            match now() {
                Ok(at) => core.cancel(Some("SCOPE_INTERRUPTED"), at, core.hard_end),
                Err(_) => core.cancel(Some("SCOPE_CLOCK"), 0, 0),
            }
        }
        if let Some(endpoint) = upstream {
            match endpoint.receive(true) {
                Ok(false) => {}
                result => {
                    let reason = result
                        .err()
                        .map(|e| e.0)
                        .unwrap_or("SCOPE_PARENT_CANCELLED");
                    let end = endpoint.cancel_deadline_ns().unwrap_or(core.hard_end);
                    match now() {
                        Ok(at) => core.cancel(Some(reason), at, end),
                        Err(_) => core.cancel(Some("SCOPE_CLOCK"), 0, 0),
                    }
                }
            }
        }
        let len = core.slots.len();
        for offset in 0..len.min(SLOTS_PER_TURN) {
            let slot = &mut core.slots[(turn_start + offset) % len];
            for event in slot.edge.take_events() {
                let result = if let Some(endpoint) = upstream {
                    endpoint.metadata(&event)
                } else {
                    Endpoint::record_metadata(ledger, std::process::id(), &event).and_then(
                        |registered| {
                            if registered {
                                core.owner.borrow_mut().charge("registrations", 1)?;
                            }
                            Ok(())
                        },
                    )
                };
                if let Err(e) = result {
                    core.owner.borrow_mut().poison(e);
                }
            }
        }
    }
    /// Retain an ORIGINAL cooperative Command without setpgid or cached-PID
    /// signals. Its own entry owner must settle descendants and close the wire.
    /// Caller-specified stdin/stdout/stderr and pre_exec are preserved.
    pub fn start_cooperative(&mut self, mut command: Command) -> io::Result<SlotId> {
        self.start_cooperative_inner(&mut command, None, None)
    }
    /// Guardian-only admission ceiling. This cannot lengthen the containing
    /// owner or shorten its retained-child cleanup budget.
    #[allow(dead_code)] // The standalone guardian links its own copy of Scope.
    pub(crate) fn start_hook_core(
        &mut self,
        mut command: Command,
        leaf_end: u64,
    ) -> io::Result<SlotId> {
        self.start_cooperative_inner(&mut command, None, Some(leaf_end))
    }
    pub fn start_cooperative_with_io(
        &mut self,
        mut command: Command,
        plan: IoPlan,
    ) -> io::Result<SlotId> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        self.start_cooperative_inner(&mut command, Some(plan), None)
    }
    fn start_cooperative_inner(
        &mut self,
        command: &mut Command,
        plan: Option<IoPlan>,
        admission_ceiling: Option<u64>,
    ) -> io::Result<SlotId> {
        self.poll();
        let at = self.core.admission_clock(&mut now)?;
        let mut random = [0u8; 16];
        if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
            return Err(error("SCOPE_RANDOM"));
        }
        let nonce: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let (parent, child_end) = UnixStream::pair()?;
        let fd = unsafe { libc::fcntl(child_end.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 5) };
        if !(5..=1024).contains(&fd) {
            if fd >= 0 {
                unsafe {
                    libc::close(fd);
                }
            }
            return Err(error("SCOPE_FD"));
        }
        let inherited = unsafe { OwnedFd::from_raw_fd(fd) };
        for key in super::attachment::KEYS
            .into_iter()
            .chain(["KEITO_SOURCE_OWNER_FD", "KEITO_SOURCE_OWNER_NONCE"])
        {
            command.env_remove(key);
        }
        command
            .env("KEITO_PROCESS_OWNER_FD", "4")
            .env("KEITO_PROCESS_OWNER_NONCE", &nonce)
            .env("KEITO_PROCESS_OWNER_PID", std::process::id().to_string())
            .env("KEITO_PROCESS_OWNER_END_NS", self.core.hard_end.to_string())
            .env("KEITO_PROCESS_OWNER_CONTRACT", "2");
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(fd, 4) < 0 || libc::fcntl(4, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if fd != 4 {
                    libc::close(fd);
                }
                Ok(())
            });
        }
        let mask = super::signals::AdmissionMask::block(command)?;
        let hard_end = self.core.hard_end;
        let capacity = plan.as_ref().map(IoPlan::capacity).unwrap_or(0);
        let mut clock_failed = false;
        let result = self.core.admit(at, capacity, |owner| {
            let endpoint = Endpoint::with_owner(parent, nonce.clone(), None, owner.clone())
                .map_err(|e| error(e.0))?;
            let pipes = plan.map(IoPlan::prepare).transpose()?;
            let fresh = match now() {
                Ok(at) => at,
                Err(_) => {
                    clock_failed = true;
                    owner.borrow_mut().poison(ProtocolError("SCOPE_CLOCK"));
                    return Err(error("SCOPE_CLOCK"));
                }
            };
            let child =
                spawn_cooperative_before(hard_end, admission_ceiling, fresh, || command.spawn())?;
            // The edge owns the actual handle before identity publication.
            let mut edge = CooperativeEdge {
                child,
                endpoint,
                nonce,
                status: None,
                cancel_sent: None,
                registered: false,
                waited: false,
                first_fault: None,
                pipes,
            };
            if let Some(pipes) = &mut edge.pipes {
                if pipes
                    .attach(
                        edge.child.stdin.take(),
                        edge.child.stdout.take(),
                        edge.child.stderr.take(),
                    )
                    .is_err()
                {
                    edge.first_fault = Some("SCOPE_IO_ATTACH");
                }
            }
            if let Err(e) = edge.endpoint.set_expected_child(edge.child.id()) {
                edge.first_fault = Some(e.0);
            }
            Ok(ProductEdge::Cooperative(edge))
        });
        drop(mask);
        drop(inherited);
        drop(child_end);
        if clock_failed {
            self.core.cancel(Some("SCOPE_CLOCK"), 0, 0);
            return result;
        }
        self.poll();
        if result.is_err() {
            self.begin_cancel(ProtocolError("SCOPE_COOPERATIVE_SPAWN"), self.core.hard_end);
        }
        result
    }
    /// QA-owned700 image constructor only. Product callers use the separate
    /// compiled-pair verified-image entry and cannot supply this QA authority.
    pub fn start_owned_opaque(
        &mut self,
        command: Command,
        plan: IoPlan,
        root: &Path,
        image: &Path,
    ) -> io::Result<SlotId> {
        self.start_with_lease(command, plan, false, |owner, end| {
            GroupLease::spawn_keeper_with_policy(owner, root, image, end, CleanupPolicy::HookShort)
        })
    }
    pub(crate) fn start_verified_opaque(
        &mut self,
        command: Command,
        plan: IoPlan,
        image: &super::image::VerifiedKeeperImage,
    ) -> io::Result<SlotId> {
        self.start_verified_opaque_with_policy(command, plan, image, CleanupPolicy::HookShort)
    }
    pub(crate) fn start_verified_opaque_with_policy(
        &mut self,
        command: Command,
        plan: IoPlan,
        image: &super::image::VerifiedKeeperImage,
        policy: CleanupPolicy,
    ) -> io::Result<SlotId> {
        self.start_with_lease(command, plan, false, |owner, end| {
            if policy == CleanupPolicy::HookShort {
                GroupLease::spawn_verified_keeper(owner, image, end)
            } else {
                GroupLease::spawn_verified_keeper_with_policy(owner, image, end, policy)
            }
        })
    }
    /// Closed developer offline-client caller only; not a product policy.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn start_dev_client_opaque_until(
        &mut self,
        mut command: Command,
        plan: IoPlan,
        image: &super::image::VerifiedKeeperImage,
        selected_end: u64,
    ) -> io::Result<SlotId> {
        clear_parent_environment(&mut command);
        let at = match now() {
            Ok(at) => at,
            Err(_) => {
                self.core.cancel(Some("SCOPE_CLOCK"), 0, 0);
                return Err(error("SCOPE_CLOCK"));
            }
        };
        let capacity = plan.capacity();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let result = self
            .core
            .admit_dev_client(at, capacity, selected_end, |owner, end| {
                let pipes = plan.prepare().map_err(|_| error("SCOPE_IO_PREPARE"))?;
                let lease = GroupLease::spawn_verified_keeper_with_policy(
                    owner,
                    image,
                    end,
                    dev_client_keeper_policy(),
                )?;
                Ok(ProductEdge::Group(GroupEdge {
                    lease,
                    command: Some(command),
                    pipes,
                    stage: Stage::Ready,
                    proof: false,
                    first_fault: None,
                    published: [false; 4],
                    setup_admission: None,
                    post_join_pending: false,
                }))
            });
        if result.is_err() {
            self.begin_cancel(ProtocolError("DEV_CLIENT_ADMISSION"), selected_end);
        }
        result
    }
    #[cfg(feature = "dev-tools")]
    pub(crate) fn take_dev_client_result(&mut self, id: &SlotId) -> io::Result<CommandResult> {
        let result = self.take_result(id)?;
        let complete = self.ledgers_complete();
        if self.core.release_dev_completed(complete).is_err() {
            self.begin_cancel(ProtocolError("DEV_CLIENT_COLLECTION"), self.core.hard_end);
            return Err(error("DEV_CLIENT_COLLECTION"));
        }
        Ok(result)
    }
    fn start_with_lease(
        &mut self,
        mut command: Command,
        plan: IoPlan,
        installer: bool,
        create: impl FnOnce(SharedOwner, u64) -> io::Result<GroupLease>,
    ) -> io::Result<SlotId> {
        clear_parent_environment(&mut command);
        let setup_admission = match self.await_hook_setup_admission() {
            Ok(admission) => admission,
            Err(error) => {
                self.begin_cancel(ProtocolError("SCOPE_SETUP_GATE"), self.core.hard_end);
                return Err(error);
            }
        };
        let at = match now() {
            Ok(at) => at,
            Err(_) => {
                self.core.cancel(Some("SCOPE_CLOCK"), 0, 0);
                return Err(error("SCOPE_CLOCK"));
            }
        };
        let capacity = plan.capacity();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let hook_setup = setup_admission.is_some();
        let hard_end = if installer || hook_setup {
            self.core.active_end()
        } else {
            self.core.hard_end
        };
        let create_edge = |owner: SharedOwner, end| {
            // Scope capacity/slot admission precedes buffer allocation, and
            // all buffers exist before spawning the first owned handle.
            let pipes = match plan.prepare() {
                Ok(pipes) => pipes,
                Err(_) => {
                    owner.borrow_mut().poison(ProtocolError("SCOPE_IO_PREPARE"));
                    return Err(error("SCOPE_IO_PREPARE"));
                }
            };
            let lease = create(owner, end)?;
            Ok(ProductEdge::Group(GroupEdge {
                lease,
                command: Some(command),
                pipes,
                stage: Stage::Ready,
                proof: false,
                first_fault: None,
                published: [false; 4],
                setup_admission,
                post_join_pending: false,
            }))
        };
        let result = if installer {
            self.core.admit_installer(at, capacity, create_edge)
        } else if hook_setup {
            self.core.admit_before(at, capacity, hard_end, create_edge)
        } else {
            self.core
                .admit(at, capacity, |owner| create_edge(owner, hard_end))
        };
        if result.is_err() {
            if self.core.owner.borrow().first_fault.is_none() {
                self.core.poison("SCOPE_ADMISSION");
            }
            match now() {
                Ok(at) => self.core.cancel(None, at, hard_end),
                Err(_) => self.core.cancel(Some("SCOPE_CLOCK"), 0, 0),
            }
        }
        result
    }
    pub(crate) fn run_verified(
        &mut self,
        command: Command,
        plan: IoPlan,
        image: &super::image::VerifiedKeeperImage,
    ) -> io::Result<CommandResult> {
        self.run_verified_with_policy(command, plan, image, CleanupPolicy::HookShort)
    }
    pub(crate) fn run_verified_with_policy(
        &mut self,
        command: Command,
        plan: IoPlan,
        image: &super::image::VerifiedKeeperImage,
        policy: CleanupPolicy,
    ) -> io::Result<CommandResult> {
        let started = if policy == CleanupPolicy::HookShort {
            self.start_verified_opaque(command, plan, image)
        } else {
            self.start_verified_opaque_with_policy(command, plan, image, policy)
        };
        self.run_started(started)
    }
    pub(crate) fn run_installer_verified(
        &mut self,
        command: Command,
        plan: IoPlan,
        image: &super::image::VerifiedKeeperImage,
    ) -> io::Result<CommandResult> {
        let started = self.start_with_lease(command, plan, true, |owner, end| {
            GroupLease::spawn_verified_keeper_with_policy(
                owner,
                image,
                end,
                CleanupPolicy::CommandProbe,
            )
        });
        self.run_started(started)
    }
    /// Fixed read-only repository cohort, used only by the HookShort owner.
    /// Three Git facts use two commands; no shell or worker thread.
    pub(crate) fn run_verified_repository(
        &mut self,
        commands: [Command; 2],
        image: &super::image::VerifiedKeeperImage,
    ) -> io::Result<[CommandResult; 2]> {
        let result = (|| {
            // All IO plans exist before the first child. Actual allocation and
            // capacity reservation still precede each registered edge's spawn.
            let plans = [
                IoPlan::new(Vec::new(), 16384, 8192)?,
                IoPlan::new(Vec::new(), 1024 * 1024, 8192)?,
            ];
            let mut slots = [None, None];
            for ((command, plan), slot) in commands.into_iter().zip(plans).zip(&mut slots) {
                // The private fixed cohort has at most two edges here.
                // A blocking sibling spawn must not consume an earlier
                // keeper's already-running short setup window.
                let upstream = &mut self.upstream;
                let ledger = &mut self.ledger;
                await_repository_admission(
                    &mut self.core,
                    &mut |core| Self::poll_turn(core, upstream, ledger),
                    &mut |core| {
                        core.wait_repository_setup(&mut now, &mut |fds, timeout| {
                            let result = unsafe {
                                libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout)
                            };
                            if result < 0 {
                                Err(io::Error::last_os_error())
                            } else {
                                Ok(result)
                            }
                        })
                    },
                )?;
                *slot = Some(self.start_verified_opaque(command, plan, image)?);
                self.poll();
                if let Some(reason) = self.core.owner.borrow().first_fault {
                    return Err(error(reason.0));
                }
            }
            let slots = slots.map(|slot| slot.expect("two admitted repository slots"));
            loop {
                self.poll();
                if let Some(results) = self.core.take_repository_results(&slots)? {
                    return Ok(results);
                }
                self.wait_command_progress()?;
            }
        })();
        if result.is_err() {
            // Includes partial admission and result validation faults. All
            // returned handles remain retained in this same cancelled Scope.
            self.finish();
        }
        result
    }
    fn run_started(&mut self, started: io::Result<SlotId>) -> io::Result<CommandResult> {
        let slot = match started {
            Ok(slot) => slot,
            Err(error) => {
                self.finish();
                return Err(error);
            }
        };
        loop {
            self.poll();
            let fault = self.core.owner.borrow().first_fault;
            if let Some(reason) = fault {
                self.finish();
                return Err(error(reason.0));
            }
            if self.core.slots[slot.index].accepted_at.is_some() {
                return self.take_result(&slot);
            }
            if let Err(error) = self.wait_command_progress() {
                self.finish();
                return Err(error);
            }
        }
    }
    fn wait_command_progress(&mut self) -> io::Result<()> {
        // Exhausted upstream read/parse budgets require the next ORIGINAL
        // pump/turn before any additional wait, with no cursor reset.
        let work = self.upstream.as_ref().map(|endpoint| &endpoint.last_poll);
        wait_with_upstream_budget(work, || {
            self.core.wait_command_progress(
                &mut now,
                &mut |fds, timeout| {
                    let result =
                        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
                    if result < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(result)
                    }
                },
                &mut || std::thread::sleep(std::time::Duration::from_millis(1)),
            )
        })
    }
    /// A bounded hot-path turn. Full retained reports are explicit snapshots,
    /// never an allocation/whole-registry scan on every command I/O poll.
    pub fn poll(&mut self) {
        Self::poll_turn(&mut self.core, &mut self.upstream, &mut self.ledger);
    }
    fn poll_turn<E: Edge>(
        core: &mut Core<E>,
        upstream: &mut Option<Endpoint>,
        ledger: &mut BTreeMap<Registration, bool>,
    ) {
        let start = core.cursor;
        Self::pump(core, upstream, ledger, start);
        core.poll(&mut now);
        if poll_joined_after_drain(core, start, &mut now, &mut |core| {
            Self::pump(core, upstream, ledger, start);
            upstream_park_probe_allowed(upstream.as_ref().map(|endpoint| &endpoint.last_poll))
        }) {
            Self::pump(core, upstream, ledger, start);
        }
    }
    pub fn request_complete(&mut self, id: &SlotId) -> io::Result<()> {
        let result = match now() {
            Ok(at) => self.core.request_complete(id, at),
            Err(_) => {
                self.core.cancel(Some("SCOPE_CLOCK"), 0, 0);
                return Err(error("SCOPE_CLOCK"));
            }
        };
        if result.is_err() {
            match now() {
                Ok(at) => self.core.cancel(None, at, self.core.hard_end),
                Err(_) => self.core.cancel(Some("SCOPE_CLOCK"), 0, 0),
            }
        }
        result
    }
    pub fn take_result(&mut self, id: &SlotId) -> io::Result<CommandResult> {
        let result = self.core.take_result(id);
        if result.is_err() {
            match now() {
                Ok(at) => self.core.cancel(None, at, self.core.hard_end),
                Err(_) => self.core.cancel(Some("SCOPE_CLOCK"), 0, 0),
            }
        }
        result
    }
    pub fn begin_cancel(&mut self, reason: ProtocolError, requested_end: u64) {
        match now() {
            Ok(at) => self.core.cancel(Some(reason.0), at, requested_end),
            Err(_) => self.core.cancel(Some("SCOPE_CLOCK"), 0, 0),
        }
    }
    pub fn finish(&mut self) -> ScopeReport {
        let upstream = &mut self.upstream;
        let ledger = &mut self.ledger;
        let mut before = true;
        let mut turn_start = self.core.cursor;
        self.core.finish_with_progress(
            &mut now,
            &mut || {
                std::thread::sleep(std::time::Duration::from_millis(1));
            },
            &mut |core| {
                if before {
                    turn_start = core.cursor;
                }
                Self::pump(core, upstream, ledger, turn_start);
                before = !before;
            },
        );
        if self.ledger.values().any(|waited| !*waited) {
            self.core.poison("SCOPE_ACTIVE_REGISTRATION");
        }
        self.report()
    }
    /// Separate from finish so the existing durable ACK is written only after
    /// resources settle, and BEFORE the ownership terminal/physical EOF.
    pub fn publish_terminal(&mut self, operation_clean: bool) -> bool {
        self.publish_terminal_with_clock(operation_clean, &mut now)
    }
    fn publish_terminal_with_clock(
        &mut self,
        operation_clean: bool,
        clock: &mut impl FnMut() -> io::Result<u64>,
    ) -> bool {
        if self.terminal_sent {
            return false;
        }
        let start = self.core.cursor;
        Self::pump(&mut self.core, &mut self.upstream, &mut self.ledger, start);
        self.terminal_sent = true;
        let timed_installer = self.core.installer.is_some();
        let gate = !timed_installer || (self.ledgers_complete() && self.core.terminal_fresh(clock));
        let clean = gate && self.report().clean && operation_clean;
        if !clean {
            self.core.poison("SCOPE_OPERATION_FAILED");
        }
        if let Some(endpoint) = &mut self.upstream {
            if let Err(e) = endpoint.send_terminal(clean) {
                self.core.owner.borrow_mut().poison(e);
                return false;
            }
        }
        clean && (!timed_installer || self.core.terminal_fresh(clock))
    }
    pub fn report(&self) -> ScopeReport {
        let first_fault = self.core.owner.borrow().first_fault;
        let all_resources = self.core.all_resources();
        ScopeReport {
            first_fault,
            admission_closed: self.core.owner.borrow().admission_closed,
            finish_called: self.core.finish_called,
            all_resources,
            clean: self.core.finished
                && self.core.finish_called
                && all_resources
                && first_fault.is_none(),
            slots: self
                .core
                .slots
                .iter()
                .enumerate()
                .map(|(slot, s)| {
                    let facts = s.edge.facts();
                    SlotReport {
                        slot,
                        status: facts.status,
                        waits_known: facts.waits_known,
                        group_absent: facts.group_absent,
                        terminal_eof: facts.terminal_eof,
                        io_required: facts.io_required,
                        io: facts.io,
                        accepted_at: s.accepted_at,
                        completion_end: s.completion_end,
                        first_fault: facts.first_fault,
                        #[cfg(feature = "dev-tools")]
                        setup_trace: s.edge.setup_trace(),
                    }
                })
                .collect(),
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if !self.core.finished {
            self.begin_cancel(ProtocolError("SCOPE_DROPPED"), self.core.hard_end);
            self.finish();
        }
        if self.upstream.is_some() && !self.terminal_sent {
            self.publish_terminal(false);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cooperative_late_preparation_cannot_lose_a_tighter_hook_admission_ceiling() {
        use std::cell::Cell;
        let spawned = Cell::new(false);
        // Preparation began within the leaf window but returned after its
        // admission ceiling, still before the longer guard's admission end.
        let result = spawn_cooperative_before(600_000_000, Some(500_000_000), 450_000_000, || {
            spawned.set(true);
            Ok(())
        });
        assert!(result.is_err());
        assert!(!spawned.get(), "late prepared command was spawned");
        for (hard, selected, fresh, allowed) in [
            (600_000_000, Some(500_000_000), 449_999_999, true),
            (600_000_000, Some(500_000_000), 525_000_000, false),
            (400_000_000, Some(500_000_000), 350_000_000, false),
            (400_000_000, Some(u64::MAX), 350_000_000, false),
            (600_000_000, None, 549_999_999, true),
            (600_000_000, None, 550_000_000, false),
            (600_000_000, Some(0), 0, false),
        ] {
            spawned.set(false);
            let result = spawn_cooperative_before(hard, selected, fresh, || {
                spawned.set(true);
                Ok(())
            });
            assert_eq!(result.is_ok(), allowed);
            assert_eq!(spawned.get(), allowed);
        }
    }
    #[cfg(feature = "dev-tools")]
    #[test]
    fn offline_client_completed_leaf_cache_releases_only_after_proven_collection() {
        let mut core = Core::new(std::rc::Rc::default(), 4_000_000_000);
        let slot = core
            .admit_dev_client(100, 0, 1_000_000_100, |_, _| {
                Ok(fake("collected", std::rc::Rc::default()))
            })
            .unwrap();
        core.request_complete(&slot, 200).unwrap();
        core.poll(&mut || Ok(201));
        core.take_result(&slot).unwrap();
        core.release_dev_completed(true).unwrap();
        assert_eq!(core.hard_end, 4_000_000_000);
        assert_eq!(core.slots[0].hard_end, 1_000_000_100);
        assert_eq!(core.active_end(), 4_000_000_000);
        assert!(core
            .admit_dev_client(1_500_000_000, 0, 2_500_000_000, |_, _| Ok(fake(
                "next",
                std::rc::Rc::default()
            )))
            .is_ok());
    }
    #[cfg(feature = "dev-tools")]
    #[test]
    fn offline_client_incomplete_or_unforwarded_collection_cannot_release_cache() {
        let mut core = Core::new(std::rc::Rc::default(), 4_000_000_000);
        core.admit_dev_client(100, 0, 1_000_000_100, |_, _| {
            Ok(fake("unproved", std::rc::Rc::default()))
        })
        .unwrap();
        assert!(core.release_dev_completed(false).is_err());
        assert_eq!(core.active_end(), 1_000_000_100);
    }
    #[cfg(feature = "dev-tools")]
    #[test]
    fn offline_client_slot_and_keeper_share_the_earlier_leaf_end() {
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let captured = std::cell::Cell::new(0);
        let mut core = Core::new(std::rc::Rc::default(), 4_000_000_000);
        core.admit_dev_client(100, 8, 1_000_000_100, |_, end| {
            captured.set(end);
            Ok(fake("offline", events.clone()))
        })
        .unwrap();
        assert_eq!(core.hard_end, 4_000_000_000);
        assert_eq!(core.slots[0].hard_end, 1_000_000_100);
        assert_eq!(captured.get(), 1_000_000_100);
    }
    #[cfg(feature = "dev-tools")]
    #[test]
    fn offline_client_does_not_admit_a_successor_with_unsettled_previous_slot() {
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut core = Core::new(std::rc::Rc::default(), 4_000_000_000);
        core.admit_dev_client(100, 0, 1_000_000_100, |_, _| {
            Ok(fake("first", events.clone()))
        })
        .unwrap();
        let created = std::cell::Cell::new(false);
        assert!(core
            .admit_dev_client(200, 0, 1_000_000_200, |_, _| {
                created.set(true);
                Ok(fake("second", events.clone()))
            })
            .is_err());
        assert!(!created.get());
        assert_eq!(core.slots.len(), 1);
    }
    #[cfg(feature = "dev-tools")]
    #[test]
    fn offline_client_parent_end_and_closed_four_slot_maximum_are_enforced() {
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut core = Core::new(std::rc::Rc::default(), 700_000_000);
        for i in 0..4 {
            core.admit_dev_client(100, 0, 1_000_000_100, |_, end| {
                assert_eq!(end, 700_000_000);
                Ok(fake("parent", events.clone()))
            })
            .unwrap();
            core.slots[i].accepted_at = Some(101);
        }
        assert!(core
            .admit_dev_client(100, 0, 1_000_000_100, |_, _| Ok(fake(
                "fifth",
                events.clone()
            )))
            .is_err());
    }
    use super::*;
    use std::{cell::RefCell, os::unix::process::ExitStatusExt};
    #[test]
    fn native_repair_admission_clock_failure_zeroes_retained_sibling() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut core = Core::new(Rc::default(), 500_000_000);
        core.admit(1, 0, |_| Ok(fake("a", events.clone()))).unwrap();
        assert!(core
            .admission_clock(&mut || Err(error("INERT_CLOCK")))
            .is_err());
        assert_eq!(core.cancel_end, Some(0));
        assert_eq!(core.slots[0].completion_end, Some(0));
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_CLOCK"))
        );
    }
    #[test]
    fn native_repair_unattached_entry_does_not_install_signal_handlers() {
        let source = include_str!("scope.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        let entry = source
            .split("pub fn entry_from_environment")
            .nth(1)
            .unwrap()
            .split("pub fn hard_end")
            .next()
            .unwrap();
        assert!(
            entry.find("attachment::consume").unwrap() < entry.find("signals::install").unwrap()
        );
    }
    #[test]
    fn native_repair_public_metadata_turn_does_not_scan_all_history() {
        let source = include_str!("scope.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        assert!(!source.contains("for slot in &mut core.slots"));
    }
    #[test]
    fn native_repair_dispatch_enters_durable_guard_without_preparation_child() {
        let source = include_str!("../bin/keito-capture-guard.rs");
        assert!(!source.contains("fn dispatch_guard("));
        let guardian = include_str!("../native_capture_guard.rs");
        assert!(
            guardian.find("let initial = marker.metadata()").unwrap()
                < guardian.find("Scope::entry_from_environment").unwrap()
        );
    }
    #[test]
    fn native_repair_actual_public_turn_bounds_metadata_and_resource_visits_together() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut core = Core::new(Rc::default(), now().unwrap() + 500_000_000);
        for name in ["a", "b", "c", "d", "e", "f", "g", "h", "i"] {
            core.admit(1, 0, |_| Ok(fake(name, events.clone())))
                .unwrap();
        }
        core.slots[2].accepted_at = Some(1);
        Scope::poll_turn(&mut core, &mut None, &mut BTreeMap::new());
        let first = events.borrow().clone();
        assert!(!first.iter().any(|event| event.ends_with(":i")));
        assert_eq!(
            first
                .iter()
                .filter(|event| event.starts_with("metadata:"))
                .count(),
            16
        );
        assert_eq!(
            first
                .iter()
                .filter(|event| event.starts_with("poll:"))
                .count(),
            7
        );
        assert_eq!(core.cursor, 8);
        events.borrow_mut().clear();
        Scope::poll_turn(&mut core, &mut None, &mut BTreeMap::new());
        assert!(events.borrow().iter().any(|event| event == "metadata:i"));
        assert!(events.borrow().iter().any(|event| event == "poll:i"));
        assert_eq!(core.slots.len(), 9);
        assert_eq!(core.slots[2].accepted_at, Some(1));
    }
    struct Fake {
        progress_sources: Vec<(i32, i16)>,
        progress_end: Option<u64>,
        progress_buffered: bool,
        #[cfg(feature = "capture-profile")]
        repository_stage: Option<crate::capture_profile::Phase>,
        events: Rc<RefCell<Vec<String>>>,
        name: &'static str,
        facts: EdgeFacts,
        begun: bool,
        fail_begin: bool,
        begin_ends: Vec<u64>,
        setup_pending: bool,
        setup_wait_source: Option<(i32, u64)>,
        setup_buffered_event: bool,
        post_join_pending: bool,
        post_join_fault: Option<&'static str>,
        #[cfg(feature = "dev-tools")]
        setup_trace: Option<super::super::group::SetupTrace>,
    }
    impl Edge for Fake {
        fn progress_wait_sources(&self, descriptors: &mut Vec<libc::pollfd>) -> Option<u64> {
            descriptors.extend(
                self.progress_sources
                    .iter()
                    .map(|(fd, events)| libc::pollfd {
                        fd: *fd,
                        events: *events,
                        revents: 0,
                    }),
            );
            self.progress_end
        }
        fn progress_buffered_event(&self) -> bool {
            self.progress_buffered
        }
        #[cfg(feature = "capture-profile")]
        fn repository_profile_stage(&self) -> Option<crate::capture_profile::Phase> {
            self.repository_stage
        }
        #[cfg(feature = "dev-tools")]
        fn setup_trace(&self) -> Option<super::super::group::SetupTrace> {
            self.setup_trace
        }
        fn setup_pending(&self) -> bool {
            self.setup_pending
        }
        fn setup_wait_source(&self) -> Option<(i32, u64)> {
            self.setup_wait_source
        }
        fn setup_buffered_event(&self) -> bool {
            self.setup_buffered_event
        }
        fn post_join_pending(&self) -> bool {
            self.post_join_pending
        }
        fn take_events(&mut self) -> Vec<String> {
            self.events
                .borrow_mut()
                .push(format!("metadata:{}", self.name));
            Vec::new()
        }
        fn begin(&mut self, end: u64) -> io::Result<()> {
            self.events
                .borrow_mut()
                .push(format!("begin:{}", self.name));
            self.begun = true;
            self.begin_ends.push(end);
            if self.fail_begin {
                Err(error("SCOPE_BEGIN"))
            } else {
                Ok(())
            }
        }
        fn poll(&mut self) -> io::Result<()> {
            self.events.borrow_mut().push(format!("poll:{}", self.name));
            self.post_join_pending = false;
            if let Some(reason) = self.post_join_fault {
                self.facts.first_fault = Some(reason);
                return Err(error(reason));
            }
            if self.begun {
                self.facts.status = Some(ExitStatus::from_raw(0));
                self.facts.waits_known = true;
                self.facts.resource_proof = true;
                self.facts.group_absent = true;
                self.facts.terminal_eof = true;
            }
            Ok(())
        }
        fn facts(&self) -> EdgeFacts {
            self.facts
        }
        fn take_output(&mut self) -> CommandOutput {
            CommandOutput {
                stdout: vec![],
                stderr: vec![],
            }
        }
    }
    fn fake(name: &'static str, events: Rc<RefCell<Vec<String>>>) -> Fake {
        Fake {
            progress_sources: Vec::new(),
            progress_end: None,
            progress_buffered: false,
            #[cfg(feature = "capture-profile")]
            repository_stage: None,
            name,
            events,
            facts: EdgeFacts::default(),
            begun: false,
            fail_begin: false,
            begin_ends: vec![],
            setup_pending: false,
            setup_wait_source: None,
            setup_buffered_event: false,
            post_join_pending: false,
            post_join_fault: None,
            #[cfg(feature = "dev-tools")]
            setup_trace: None,
        }
    }
    #[test]
    #[cfg(feature = "dev-tools")]
    fn failure_setup_trace_never_substitutes_a_healthy_sibling_or_unmatched_fault() {
        use super::super::group::SetupTrace;
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let healthy = core
            .admit(1, 32, |_| Ok(fake("healthy", Rc::default())))
            .unwrap();
        core.slots[healthy.index].edge.setup_trace = Some(SetupTrace {
            setup_start_ns: 10,
            ready_frame_observed: true,
            ..Default::default()
        });
        assert!(core.first_fault_setup_trace().is_none());
        core.poison("GROUP_DEADLINE");
        assert!(core.first_fault_setup_trace().is_none());
        core.slots[healthy.index]
            .edge
            .setup_trace
            .as_mut()
            .unwrap()
            .fault_origin = Some(("GROUP_IO", "JOINED"));
        assert!(core.first_fault_setup_trace().is_none());
    }

    #[test]
    #[cfg(feature = "dev-tools")]
    fn failure_setup_trace_selects_the_origin_after_a_healthy_first_slot() {
        use super::super::group::SetupTrace;
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let healthy = core
            .admit(1, 32, |_| Ok(fake("healthy", Rc::default())))
            .unwrap();
        let failed = core
            .admit(2, 32, |_| Ok(fake("failed", Rc::default())))
            .unwrap();
        core.slots[healthy.index].edge.setup_trace = Some(SetupTrace {
            setup_start_ns: 10,
            ready_frame_observed: true,
            ..Default::default()
        });
        core.slots[failed.index].edge.setup_trace = Some(SetupTrace {
            setup_start_ns: 20,
            fault_origin: Some(("GROUP_DEADLINE", "AWAIT_READY")),
            ..Default::default()
        });
        core.poison("GROUP_DEADLINE");
        let selected = core.first_fault_setup_trace().unwrap();
        assert_eq!(selected.setup_start_ns, 20);
        assert_eq!(
            selected.fault_origin,
            Some(("GROUP_DEADLINE", "AWAIT_READY"))
        );
        assert!(!selected.ready_frame_observed);
    }

    #[test]
    fn hook_setup_gate_is_exclusive_cloexec_and_rejects_replaced_or_unsafe_files() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = std::fs::File::open(temp.path()).unwrap();
        let mut first = HookSetupAdmission::open(&directory, 0).unwrap();
        assert!(first.try_lock(&directory).unwrap());
        assert_ne!(
            unsafe { libc::fcntl(first.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let mut second = HookSetupAdmission::open(&directory, 0).unwrap();
        assert!(!second.try_lock(&directory).unwrap());
        drop(first);
        assert!(second.try_lock(&directory).unwrap());
        let path = temp.path().join("capture-hook-setup-0.lock");
        std::fs::rename(&path, temp.path().join("old-lock")).unwrap();
        let replacement = HookSetupAdmission::open(&directory, 0).unwrap();
        assert!(second.try_lock(&directory).is_err());
        drop(second);
        drop(replacement);
        std::fs::remove_file(&path).unwrap();
        let target = temp.path().join("owned-target");
        std::fs::write(&target, b"PRIVATE_UNREAD_TARGET").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(HookSetupAdmission::open(&directory, 0).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"not-empty").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(HookSetupAdmission::open(&directory, 0).is_err());
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(HookSetupAdmission::open(&directory, 0).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&path, temp.path().join("extra-link")).unwrap();
        assert!(HookSetupAdmission::open(&directory, 0).is_err());
        std::fs::remove_file(temp.path().join("extra-link")).unwrap();
        std::fs::remove_file(&path).unwrap();
        let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(HookSetupAdmission::open(&directory, 0).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(HookSetupAdmission::open(&directory, 0).is_err());
    }

    #[test]
    fn hook_setup_pool_has_exactly_four_independent_slots_and_releases_each_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = File::open(temp.path()).unwrap();
        let mut held = Vec::new();
        for slot in 0..HOOK_SETUP_SLOTS {
            let mut admission = HookSetupAdmission::open(&directory, slot).unwrap();
            assert!(admission.try_lock(&directory).unwrap());
            held.push(admission);
        }
        assert!(HookSetupAdmission::open(&directory, HOOK_SETUP_SLOTS).is_err());
        for slot in 0..HOOK_SETUP_SLOTS {
            let mut waiter = HookSetupAdmission::open(&directory, slot).unwrap();
            assert!(!waiter.try_lock(&directory).unwrap());
        }
        drop(held.remove(2));
        let mut released = HookSetupAdmission::open(&directory, 2).unwrap();
        assert!(released.try_lock(&directory).unwrap());
        let mut still_held = HookSetupAdmission::open(&directory, 0).unwrap();
        assert!(!still_held.try_lock(&directory).unwrap());
    }

    #[test]
    fn guard_core_pool_is_four_independent_cloexec_locks_without_consuming_leaf_capacity() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = File::open(temp.path()).unwrap();
        let mut held = Vec::new();
        for slot in 0..HOOK_SETUP_SLOTS {
            let mut permit =
                HookSetupAdmission::open_kind(&directory, slot, HookCapacityKind::Core).unwrap();
            assert!(permit.try_lock(&directory).unwrap());
            assert_ne!(
                unsafe { libc::fcntl(permit.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
            let mut leaf = HookSetupAdmission::open(&directory, slot).unwrap();
            assert_ne!(
                permit.file.metadata().unwrap().ino(),
                leaf.file.metadata().unwrap().ino()
            );
            assert!(
                leaf.try_lock(&directory).unwrap(),
                "core holder consumed a leaf permit"
            );
            held.push(permit);
        }
        assert!(HookSetupAdmission::open_kind(
            &directory,
            HOOK_SETUP_SLOTS,
            HookCapacityKind::Core
        )
        .is_err());
        for attempt in 0..20 {
            let mut waiter = HookSetupAdmission::open_kind(
                &directory,
                attempt % HOOK_SETUP_SLOTS,
                HookCapacityKind::Core,
            )
            .unwrap();
            assert!(
                !waiter.try_lock(&directory).unwrap(),
                "fifth core admitted into a full pool"
            );
        }
        drop(held.remove(2));
        let mut next =
            HookSetupAdmission::open_kind(&directory, 2, HookCapacityKind::Core).unwrap();
        assert!(next.try_lock(&directory).unwrap());
        let mut blocked =
            HookSetupAdmission::open_kind(&directory, 0, HookCapacityKind::Core).unwrap();
        assert!(!blocked.try_lock(&directory).unwrap());
    }

    #[test]
    fn guard_core_capacity_ceiling_preserves_earlier_parent_and_clock_failure() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 100_000_000);
        assert!(await_hook_setup_capacity_until(
            &mut core,
            &mut |_| {},
            &mut || Ok(50_000_000),
            &mut || panic!("earlier parent ignored"),
            &mut || panic!("expired parent wait"),
            Some(500_000_000)
        )
        .is_err());
        assert_eq!(core.hard_end, 100_000_000);
        let mut core: Core<Fake> = Core::new(Rc::default(), 600_000_000);
        assert!(await_hook_setup_capacity_until(
            &mut core,
            &mut |_| {},
            &mut || Err(error("SCOPE_CLOCK")),
            &mut || panic!("clock failure admitted"),
            &mut || panic!("failed clock wait"),
            Some(500_000_000)
        )
        .is_err());
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_CLOCK"))
        );
        assert_eq!(core.cancel_end, Some(0));
        assert!(core.owner.borrow().admission_closed);
    }

    fn completed_hook_facts() -> EdgeFacts {
        EdgeFacts {
            waits_known: true,
            group_absent: true,
            terminal_eof: true,
            resource_proof: true,
            io_required: true,
            io: IoFacts {
                stdin_closed: true,
                stdout_eof: true,
                stderr_eof: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn hook_admission_remains_locked_through_running_and_cleanup() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = File::open(temp.path()).unwrap();
        let mut admission = Some(HookSetupAdmission::open(&directory, 0).unwrap());
        assert!(admission.as_mut().unwrap().try_lock(&directory).unwrap());
        let mut waiter = HookSetupAdmission::open(&directory, 0).unwrap();
        for stage in [Stage::Ready, Stage::Park, Stage::Running, Stage::Cleaning] {
            release_hook_admission(&stage, completed_hook_facts(), &mut admission);
            assert!(admission.is_some(), "live command released capacity");
            assert!(!waiter.try_lock(&directory).unwrap());
        }
        release_hook_admission(&Stage::Done, completed_hook_facts(), &mut admission);
        assert!(admission.is_none());
        assert!(waiter.try_lock(&directory).unwrap());
        // Completion polling cannot release a second description's lock.
        release_hook_admission(&Stage::Done, completed_hook_facts(), &mut admission);
        let mut next = HookSetupAdmission::open(&directory, 0).unwrap();
        assert!(!next.try_lock(&directory).unwrap());
    }

    #[test]
    fn hook_admission_requires_full_wait_group_and_pipe_finality() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = File::open(temp.path()).unwrap();
        let mut admission = Some(HookSetupAdmission::open(&directory, 0).unwrap());
        assert!(admission.as_mut().unwrap().try_lock(&directory).unwrap());
        let mut waiter = HookSetupAdmission::open(&directory, 0).unwrap();
        for missing in 0..7 {
            let mut facts = completed_hook_facts();
            match missing {
                0 => facts.resource_proof = false,
                1 => facts.waits_known = false,
                2 => facts.group_absent = false,
                3 => facts.terminal_eof = false,
                4 => facts.io.stdin_closed = false,
                5 => facts.io.stdout_eof = false,
                _ => facts.io.stderr_eof = false,
            }
            release_hook_admission(&Stage::Done, facts, &mut admission);
            assert!(admission.is_some(), "incomplete cleanup released capacity");
            assert!(!waiter.try_lock(&directory).unwrap());
        }
        release_hook_admission(&Stage::Done, completed_hook_facts(), &mut admission);
        assert!(admission.is_none());
        assert!(waiter.try_lock(&directory).unwrap());
    }

    #[test]
    fn hook_setup_capacity_progresses_and_never_renews_an_expired_or_tighter_end() {
        use std::cell::Cell;
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let polls = Cell::new(0);
        let attempts = Cell::new(0);
        let pauses = Cell::new(0);
        await_hook_setup_capacity(
            &mut core,
            &mut |_| polls.set(polls.get() + 1),
            &mut || Ok(100),
            &mut || {
                attempts.set(attempts.get() + 1);
                Ok(attempts.get() == 2)
            },
            &mut || pauses.set(pauses.get() + 1),
        )
        .unwrap();
        assert!(polls.get() >= 3);
        assert_eq!(attempts.get(), 2);
        assert_eq!(pauses.get(), 1);
        assert_eq!(core.hard_end, 500_000_000);
        for (hard_end, at) in [(500_000_000, 450_000_000), (100_000_000, 50_000_000)] {
            let mut core: Core<Fake> = Core::new(Rc::default(), hard_end);
            await_hook_setup_capacity(
                &mut core,
                &mut |_| {},
                &mut || Ok(at),
                &mut || panic!("expired setup admission attempted"),
                &mut || panic!("expired pause"),
            )
            .unwrap_err();
            assert_eq!(core.hard_end, hard_end);
            assert!(core.owner.borrow().first_fault.is_some());
        }
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let reads = Cell::new(0);
        await_hook_setup_capacity(
            &mut core,
            &mut |_| {},
            &mut || {
                reads.set(reads.get() + 1);
                Ok(if reads.get() == 1 { 1 } else { 450_000_000 })
            },
            &mut || Ok(true),
            &mut || panic!("locked pause"),
        )
        .unwrap_err();
        assert!(core.owner.borrow().first_fault.is_some());
    }

    #[test]
    fn guard_core_capacity_refuses_at_the_original_leaf_ceiling_before_lock() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 600_000_000);
        let attempts = std::cell::Cell::new(0);
        let result = await_hook_setup_capacity_until(
            &mut core,
            &mut |_| {},
            &mut || Ok(450_000_000),
            &mut || {
                attempts.set(attempts.get() + 1);
                Ok(true)
            },
            &mut || panic!("expired core capacity wait"),
            Some(500_000_000),
        );
        assert!(
            result.is_err(),
            "guard's longer end admitted an expired leaf"
        );
        assert_eq!(attempts.get(), 0);
        assert_eq!(core.hard_end, 600_000_000);
    }

    #[test]
    fn guard_core_capacity_rechecks_leaf_ceiling_after_lock() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 600_000_000);
        let reads = std::cell::Cell::new(0);
        let result = await_hook_setup_capacity_until(
            &mut core,
            &mut |_| {},
            &mut || {
                reads.set(reads.get() + 1);
                Ok(if reads.get() == 1 { 1 } else { 450_000_000 })
            },
            &mut || Ok(true),
            &mut || panic!("late acquired core permit"),
            Some(500_000_000),
        );
        assert!(
            result.is_err(),
            "lock wait renewed the original leaf ceiling"
        );
        assert_eq!(reads.get(), 2);
        assert_eq!(core.hard_end, 600_000_000);
    }

    #[test]
    fn hook_setup_capacity_rechecks_parent_after_lock_and_preserves_gate_errors() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let owner = core.owner.clone();
        await_hook_setup_capacity(
            &mut core,
            &mut |_| {},
            &mut || Ok(1),
            &mut || {
                owner.borrow_mut().poison(ProtocolError("PARENT_CANCELLED"));
                Ok(true)
            },
            &mut || panic!("post-lock cancelled pause"),
        )
        .unwrap_err();
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("PARENT_CANCELLED"))
        );
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        await_hook_setup_capacity(
            &mut core,
            &mut |_| {},
            &mut || Ok(1),
            &mut || Err(error("PRIVATE_IO_CANARY")),
            &mut || panic!("failed gate pause"),
        )
        .unwrap_err();
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_SETUP_GATE"))
        );
    }

    #[test]
    fn hook_setup_capacity_preserves_parent_fault_clock_failure_and_poll_shortening() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        core.poison("PARENT_CANCELLED");
        await_hook_setup_capacity(
            &mut core,
            &mut |_| {},
            &mut || Ok(1),
            &mut || panic!("cancelled lock attempt"),
            &mut || {},
        )
        .unwrap_err();
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("PARENT_CANCELLED"))
        );
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        await_hook_setup_capacity(
            &mut core,
            &mut |_| {},
            &mut || Err(error("SCOPE_CLOCK")),
            &mut || panic!("clock-failed lock attempt"),
            &mut || {},
        )
        .unwrap_err();
        assert_eq!(core.cancel_end, Some(0));
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_CLOCK"))
        );
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        await_hook_setup_capacity(
            &mut core,
            &mut |core| core.hard_end = 50_000_001,
            &mut || Ok(1),
            &mut || panic!("shortened lock attempt"),
            &mut || {},
        )
        .unwrap_err();
        assert_eq!(core.hard_end, 50_000_001);
    }

    mod setup_schedule_tests {
        use super::*;
        use std::cell::Cell;

        fn pending_core(buffered: bool, fd: i32) -> (Core<Fake>, SlotId) {
            let mut core = Core::new(Rc::default(), 500_000_000);
            let id = core
                .admit(1, 32, |_| Ok(fake("joined", Rc::default())))
                .unwrap();
            core.slots[id.index].edge.setup_pending = true;
            core.slots[id.index].edge.setup_wait_source = Some((fd, 20_000_000));
            core.slots[id.index].edge.setup_buffered_event = buffered;
            (core, id)
        }

        #[test]
        fn setup_schedule_buffered_event_skips_wait_without_advancing_or_taking_output() {
            let (mut core, id) = pending_core(true, 7);
            let mut clocks = 0;
            core.wait_repository_setup(
                &mut || {
                    clocks += 1;
                    Ok(10_000_000)
                },
                &mut |_, _| panic!("complete buffered frame must return to original parser"),
            )
            .unwrap();
            assert_eq!(clocks, 1);
            assert!(core.slots[id.index].edge.setup_pending);
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(core.slots[id.index].completion_end.is_none());
            assert!(!core.slots[id.index].output_taken);
            assert_eq!(core.hard_end, 500_000_000);
        }

        #[test]
        fn setup_schedule_partial_bytes_still_wait_on_the_original_fd_and_end() {
            let (mut core, id) = pending_core(false, 7);
            let mut calls = 0;
            core.wait_repository_setup(&mut || Ok(19_950_000), &mut |fds, timeout| {
                calls += 1;
                assert_eq!(
                    (fds.len(), fds[0].fd, fds[0].events, timeout),
                    (1, 7, libc::POLLIN, 0)
                );
                Ok(0)
            })
            .unwrap();
            assert_eq!(calls, 1);
            assert!(core.slots[id.index].edge.setup_pending);
            assert!(core.slots[id.index].accepted_at.is_none());
        }

        #[test]
        fn setup_schedule_buffer_shortcut_cannot_bypass_invalid_fd_clock_or_owner() {
            let (mut invalid, _) = pending_core(true, -1);
            assert_eq!(
                invalid
                    .wait_repository_setup(
                        &mut || panic!("invalid FD refuses first"),
                        &mut |_, _| { panic!("invalid FD must not poll") }
                    )
                    .unwrap_err()
                    .to_string(),
                "SCOPE_SETUP_WAIT"
            );
            let (mut no_clock, id) = pending_core(true, 7);
            assert!(no_clock
                .wait_repository_setup(&mut || Err(error("INJECTED_CLOCK")), &mut |_, _| {
                    panic!("clock failure must not poll")
                })
                .is_err());
            assert_eq!(no_clock.cancel_end, Some(0));
            assert_eq!(no_clock.slots[id.index].completion_end, Some(0));
            for state in ["fault", "cancelled", "finished"] {
                let (mut core, id) = pending_core(true, 7);
                match state {
                    "fault" => core.poison("ORIGINAL_FAULT"),
                    "cancelled" => core.cancel_end = Some(10),
                    _ => core.finished = true,
                }
                assert!(core
                    .wait_repository_setup(
                        &mut || panic!("closed owner must refuse"),
                        &mut |_, _| { panic!("closed owner must not poll") }
                    )
                    .is_err());
                assert!(core.slots[id.index].accepted_at.is_none());
                assert!(!core.slots[id.index].output_taken);
            }
        }

        #[test]
        fn setup_schedule_exact_end_returns_to_deadline_parser_without_fake_acceptance() {
            let (mut core, id) = pending_core(true, 7);
            let mut clocks = 0;
            core.wait_repository_setup(
                &mut || {
                    clocks += 1;
                    Ok(20_000_000)
                },
                &mut |_, _| panic!("no wait beyond original end"),
            )
            .unwrap();
            assert_eq!(clocks, 1);
            assert_eq!(
                core.slots[id.index].edge.setup_wait_source,
                Some((7, 20_000_000))
            );
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].output_taken);
        }

        #[test]
        fn setup_schedule_join_retains_all_pipes_before_first_error_and_only_schedules_success() {
            for state in ["success", "start_error", "attach_error", "both_errors"] {
                let mut stage = Stage::Ready;
                let mut pending = true;
                let attached = Cell::new(false);
                let started = if state == "start_error" || state == "both_errors" {
                    Err(error("ORIGINAL_START"))
                } else {
                    Ok(())
                };
                let result = retain_group_start(&mut stage, &mut pending, started, || {
                    attached.set(true);
                    if state == "attach_error" || state == "both_errors" {
                        Err(error("PIPE_ATTACH"))
                    } else {
                        Ok(())
                    }
                });
                assert!(
                    attached.get(),
                    "post-spawn error must not bypass handle retention"
                );
                assert!(matches!(stage, Stage::Park));
                assert_eq!(pending, state == "success");
                match state {
                    "success" => assert!(result.is_ok()),
                    "attach_error" => assert_eq!(result.unwrap_err().to_string(), "PIPE_ATTACH"),
                    _ => assert_eq!(result.unwrap_err().to_string(), "ORIGINAL_START"),
                }
            }
        }

        #[test]
        fn setup_schedule_postjoin_observes_once_after_drain_without_sleep_or_result_acceptance() {
            let (mut core, id) = pending_core(false, 7);
            core.slots[id.index].edge.post_join_pending = true;
            let events = core.slots[id.index].edge.events.clone();
            let original_cursor = core.cursor;
            assert!(poll_joined_after_drain(
                &mut core,
                0,
                &mut || Ok(19_970_000),
                &mut |_| {
                    events.borrow_mut().push("drain".into());
                    true
                }
            ));
            assert_eq!(events.borrow().as_slice(), ["drain", "poll:joined"]);
            assert!(!core.slots[id.index].edge.post_join_pending);
            assert!(!poll_joined_after_drain(
                &mut core,
                0,
                &mut || panic!("no second observation"),
                &mut |_| true
            ));
            assert_eq!(core.cursor, original_cursor);
            assert_eq!(core.hard_end, 500_000_000);
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].output_taken);
        }

        #[test]
        fn setup_schedule_parent_cancel_after_spawn_wins_before_extra_park_observation() {
            let (mut core, id) = pending_core(false, 7);
            core.slots[id.index].edge.post_join_pending = true;
            let events = core.slots[id.index].edge.events.clone();
            assert!(!poll_joined_after_drain(
                &mut core,
                0,
                &mut || panic!("cancelled pass must stop"),
                &mut |core| {
                    core.cancel(Some("PARENT_CANCELLED"), 2, 3);
                    true
                }
            ));
            assert!(!events
                .borrow()
                .iter()
                .any(|event| event.starts_with("poll:")));
            assert_eq!(
                core.owner.borrow().first_fault,
                Some(ProtocolError("PARENT_CANCELLED"))
            );
            assert_eq!(core.slots[id.index].completion_end, Some(3));
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].output_taken);
        }

        #[test]
        fn setup_schedule_exhausted_upstream_drain_defers_extra_probe_without_acceptance() {
            for (reads, parsed, allowed) in [
                (0, 0, true),
                (
                    super::super::super::protocol::MAX_READS - 1,
                    super::super::super::protocol::MAX_PARSE - 1,
                    true,
                ),
                (super::super::super::protocol::MAX_READS, 0, false),
                (0, super::super::super::protocol::MAX_PARSE, false),
            ] {
                let work = super::super::super::protocol::PollWork { reads, parsed };
                assert_eq!(upstream_park_probe_allowed(Some(&work)), allowed);
                if allowed {
                    continue;
                }
                let (mut core, id) = pending_core(false, 7);
                core.slots[id.index].edge.post_join_pending = true;
                let events = core.slots[id.index].edge.events.clone();
                assert!(!poll_joined_after_drain(
                    &mut core,
                    0,
                    &mut || {
                        panic!("an exhausted upstream drain must precede any extra observation")
                    },
                    &mut |_| upstream_park_probe_allowed(Some(&work))
                ));
                assert!(core.slots[id.index].edge.post_join_pending);
                assert!(events.borrow().is_empty());
                assert!(core.slots[id.index].accepted_at.is_none());
                assert!(!core.slots[id.index].output_taken);
                // A later proved drain schedules the original Park path once.
                assert!(poll_joined_after_drain(
                    &mut core,
                    0,
                    &mut || Ok(2),
                    &mut |_| { upstream_park_probe_allowed(None) }
                ));
                assert!(!core.slots[id.index].edge.post_join_pending);
                assert_eq!(
                    events
                        .borrow()
                        .iter()
                        .filter(|event| event.starts_with("poll:"))
                        .count(),
                    1
                );
            }
        }

        #[test]
        fn setup_schedule_postjoin_failure_clock_and_exact_operation_end_remain_closed() {
            for state in ["poll", "clock", "deadline", "owner", "closed"] {
                let (mut core, id) = pending_core(false, 7);
                core.slots[id.index].edge.post_join_pending = true;
                if state == "poll" {
                    core.slots[id.index].edge.post_join_fault = Some("ORIGINAL_PARK_FAULT");
                }
                if state == "owner" {
                    core.poison("ORIGINAL_OWNER_FAULT");
                }
                if state == "closed" {
                    core.owner.borrow_mut().admission_closed = true;
                }
                let events = core.slots[id.index].edge.events.clone();
                poll_joined_after_drain(
                    &mut core,
                    0,
                    &mut || match state {
                        "clock" => Err(error("INJECTED_CLOCK")),
                        "deadline" => Ok(450_000_000),
                        _ => Ok(2),
                    },
                    &mut |_| true,
                );
                assert!(core.owner.borrow().first_fault.is_some());
                assert!(core.cancel_end.is_some());
                assert_eq!(
                    events
                        .borrow()
                        .iter()
                        .filter(|event| event.starts_with("poll:"))
                        .count(),
                    usize::from(state == "poll")
                );
                assert!(core.slots[id.index].accepted_at.is_none());
                assert!(!core.slots[id.index].output_taken);
                assert_eq!(core.hard_end, 500_000_000);
            }
        }

        #[test]
        fn setup_schedule_targeted_pass_keeps_the_original_fair_window_and_skips_ordinary_edges() {
            let events = Rc::new(RefCell::new(vec![]));
            let mut core = Core::new(Rc::default(), 500_000_000);
            let mut ids = vec![];
            for _ in 0..(SLOTS_PER_TURN + 2) {
                let id = core
                    .admit(1, 0, |_| Ok(fake("edge", events.clone())))
                    .unwrap();
                core.slots[id.index].edge.post_join_pending = true;
                ids.push(id);
            }
            core.slots[ids[2].index].edge.post_join_pending = false;
            let cursor = core.cursor;
            assert!(core.poll_post_joined(0, &mut || Ok(2)));
            assert_eq!(
                events
                    .borrow()
                    .iter()
                    .filter(|event| event.starts_with("poll:"))
                    .count(),
                SLOTS_PER_TURN - 1
            );
            assert!(core.slots[ids[SLOTS_PER_TURN].index].edge.post_join_pending);
            assert_eq!(core.cursor, cursor);
            assert!(core
                .slots
                .iter()
                .all(|slot| slot.accepted_at.is_none() && !slot.output_taken));
        }
    }

    #[test]
    fn repository_admission_pending_setup_blocks_without_waiting_for_workload_completion() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        assert!(core.repository_admission_ready().unwrap());
        let slot = core
            .admit(1, 32, |_| Ok(fake("first", Rc::default())))
            .unwrap();
        core.slots[slot.index].edge.setup_pending = true;
        let original_end = core.hard_end;
        assert!(!core.repository_admission_ready().unwrap());
        assert_eq!(core.hard_end, original_end);
        assert_eq!(core.slots.len(), 1);
        assert!(!core.slots[0].output_taken);
        assert!(core.owner.borrow().first_fault.is_none());
        core.slots[slot.index].edge.setup_pending = false;
        assert!(core.repository_admission_ready().unwrap());
        assert!(core.slots[0].accepted_at.is_none());
        assert!(core.slots[0].edge.facts.status.is_none());
    }
    #[test]
    fn repository_admission_actual_stage_mapping_only_waits_for_ready_and_park() {
        assert!(Stage::Ready.setup_pending());
        assert!(Stage::Park.setup_pending());
        assert!(!Stage::Running.setup_pending());
        assert!(!Stage::Cleaning.setup_pending());
        assert!(!Stage::Done.setup_pending());
    }
    #[test]
    fn repository_setup_wait_observes_a_response_inside_the_original_final_poll_period() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let slot = core
            .admit(1, 32, |_| Ok(fake("ready", Rc::default())))
            .unwrap();
        core.slots[slot.index].edge.setup_pending = true;
        let end = 20_000_000u64;
        core.slots[slot.index].edge.setup_wait_source = Some((7, end));
        let ready_at = end - 30_000;
        let clock = std::cell::Cell::new(end - 49_000);
        let mut turns = 0;
        let result = await_repository_admission(
            &mut core,
            &mut |core| {
                turns += 1;
                assert!(turns <= 64, "finite scheduling counterexample");
                if clock.get() >= end {
                    core.poison("GROUP_DEADLINE");
                } else if clock.get() >= ready_at {
                    core.slots[slot.index].edge.setup_pending = false;
                }
            },
            &mut |core| {
                core.wait_repository_setup(&mut || Ok(clock.get()), &mut |_, timeout| {
                    assert_eq!(timeout, 0, "do not round up a submillisecond remainder");
                    clock.set(clock.get() + 1_000);
                    Ok(0)
                })
            },
        );
        assert!(result.is_ok(), "READY before original deadline was missed");
        assert!(clock.get() >= ready_at && clock.get() < end);
        assert_eq!(core.hard_end, 500_000_000);
        assert!(core.slots[slot.index].completion_end.is_none());
        assert!(core.slots[slot.index].accepted_at.is_none());
        assert!(!core.slots[slot.index].output_taken);
    }
    #[test]
    fn repository_setup_wait_progress_fault_stops_before_another_pause() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let slot = core
            .admit(1, 32, |_| Ok(fake("ready", Rc::default())))
            .unwrap();
        core.slots[slot.index].edge.setup_pending = true;
        let mut turns = 0;
        let mut pauses = 0;
        let result = await_repository_admission(
            &mut core,
            &mut |core| {
                turns += 1;
                if turns == 2 {
                    core.poison("ORIGINAL_WAIT_FAULT");
                }
            },
            &mut |_| {
                pauses += 1;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!((turns, pauses), (2, 1));
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("ORIGINAL_WAIT_FAULT"))
        );
        assert_eq!(core.slots.len(), 1);
        assert!(!core.slots[slot.index].output_taken);
    }
    #[test]
    fn repository_setup_wait_expiry_refuses_without_a_pause_or_new_interval() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let slot = core
            .admit(1, 32, |_| Ok(fake("ready", Rc::default())))
            .unwrap();
        core.slots[slot.index].edge.setup_pending = true;
        let end = core.hard_end;
        let result = await_repository_admission(
            &mut core,
            &mut |core| core.poison("GROUP_DEADLINE"),
            &mut |_| panic!("expired setup must not wait again"),
        );
        assert!(result.is_err());
        assert_eq!(core.hard_end, end);
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("GROUP_DEADLINE"))
        );
        assert!(core.slots[slot.index].completion_end.is_none());
        assert!(core.slots[slot.index].accepted_at.is_none());
    }
    #[test]
    fn repository_setup_readiness_wait_blocks_only_within_original_ends() {
        for (setup_end, parent_end, at, expected) in [
            (20_000_000, 500_000_000, 10_000_000, Some(1)),
            (20_000_000, 500_000_000, 19_000_001, Some(0)),
            (20_000_000, 50_500_000, 0, Some(0)),
            (20_000_000, 50_000_000, 0, None),
            (20_000_000, 500_000_000, 20_000_000, None),
        ] {
            let mut core = Core::new(Rc::default(), 500_000_000);
            let id = core
                .admit(0, 0, |_| Ok(fake("wait", Rc::default())))
                .unwrap();
            core.hard_end = parent_end;
            core.slots[id.index].edge.setup_pending = true;
            core.slots[id.index].edge.setup_wait_source = Some((7, setup_end));
            let mut observed = None;
            core.wait_repository_setup(&mut || Ok(at), &mut |fds, timeout| {
                assert_eq!(fds.len(), 1);
                assert_eq!((fds[0].fd, fds[0].events), (7, libc::POLLIN));
                observed = Some(timeout);
                fds[0].revents = libc::POLLIN | libc::POLLHUP;
                Ok(1)
            })
            .unwrap();
            assert_eq!(observed, expected);
            assert_eq!(core.hard_end, parent_end);
            assert!(core.slots[id.index].edge.setup_pending);
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].output_taken);
        }
    }
    #[test]
    fn repository_setup_readiness_wait_interrupt_returns_to_progress_and_faults_refuse() {
        for condition in ["interrupt", "invalid", "error"] {
            let mut core = Core::new(Rc::default(), 500_000_000);
            let id = core
                .admit(1, 0, |_| Ok(fake("wait", Rc::default())))
                .unwrap();
            core.slots[id.index].edge.setup_pending = true;
            core.slots[id.index].edge.setup_wait_source = Some((7, 20_000_000));
            let result =
                core.wait_repository_setup(&mut || Ok(10_000_000), &mut |fds, _| match condition {
                    "interrupt" => Err(io::Error::from_raw_os_error(libc::EINTR)),
                    "invalid" => {
                        fds[0].revents = libc::POLLNVAL;
                        Ok(1)
                    }
                    _ => Err(io::Error::from_raw_os_error(libc::EIO)),
                });
            assert_eq!(result.is_ok(), condition == "interrupt");
            assert_eq!(
                core.owner.borrow().first_fault.is_none(),
                condition == "interrupt"
            );
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].output_taken);
        }
    }
    #[test]
    fn repository_setup_readiness_wait_clock_failure_cancels_existing_owner_at_zero() {
        let mut core = Core::new(Rc::default(), 500_000_000);
        let id = core
            .admit(1, 0, |_| Ok(fake("wait", Rc::default())))
            .unwrap();
        core.slots[id.index].edge.setup_pending = true;
        core.slots[id.index].edge.setup_wait_source = Some((7, 20_000_000));
        assert!(core
            .wait_repository_setup(&mut || Err(error("INJECTED_CLOCK")), &mut |_, _| panic!(
                "clock failure cannot wait"
            ),)
            .is_err());
        assert_eq!(core.cancel_end, Some(0));
        assert_eq!(core.slots[id.index].completion_end, Some(0));
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_CLOCK"))
        );
    }
    #[test]
    fn scope_wait_final_pending_setup_uses_readiness_in_the_original_last_fraction() {
        let mut core = Core::new(Rc::default(), 500_000_000);
        let id = core
            .admit(0, 0, |_| Ok(fake("last", Rc::default())))
            .unwrap();
        core.slots[id.index].edge.setup_pending = true;
        core.slots[id.index].edge.setup_wait_source = Some((7, 20_000_000));
        let mut polls = 0;
        let mut pauses = 0;
        core.wait_command_progress(
            &mut || Ok(19_950_000),
            &mut |fds, timeout| {
                polls += 1;
                assert_eq!(timeout, 0);
                assert_eq!((fds.len(), fds[0].fd, fds[0].events), (1, 7, libc::POLLIN));
                fds[0].revents = libc::POLLIN;
                Ok(1)
            },
            &mut || pauses += 1,
        )
        .unwrap();
        assert_eq!((polls, pauses), (1, 0));
        assert_eq!(core.hard_end, 500_000_000);
        assert_eq!(
            core.slots[id.index].edge.setup_wait_source,
            Some((7, 20_000_000))
        );
        assert!(core.slots[id.index].edge.setup_pending);
        assert!(core.slots[id.index].accepted_at.is_none());
    }
    #[test]
    fn scope_wait_repository_final_sibling_keeps_its_own_pending_socket() {
        let mut core = Core::new(Rc::default(), 500_000_000);
        let a = core
            .admit(0, 0, |_| Ok(fake("running", Rc::default())))
            .unwrap();
        let b = core
            .admit(0, 0, |_| Ok(fake("pending", Rc::default())))
            .unwrap();
        core.slots[b.index].edge.setup_pending = true;
        core.slots[b.index].edge.setup_wait_source = Some((9, 20_000_000));
        assert!(core.take_repository_results(&[a, b]).unwrap().is_none());
        let mut polls = 0;
        let mut pauses = 0;
        core.wait_command_progress(
            &mut || Ok(10_000_000),
            &mut |fds, timeout| {
                polls += 1;
                assert_eq!((fds.len(), fds[0].fd, timeout), (1, 9, 1));
                Ok(0)
            },
            &mut || pauses += 1,
        )
        .unwrap();
        assert_eq!((polls, pauses), (1, 0));
        assert!(core
            .slots
            .iter()
            .all(|slot| slot.accepted_at.is_none() && !slot.output_taken));
    }
    #[test]
    fn scope_wait_running_edges_keep_the_existing_ordinary_pause() {
        let mut core = Core::new(Rc::default(), 500_000_000);
        core.admit(0, 0, |_| Ok(fake("running", Rc::default())))
            .unwrap();
        let mut polls = 0;
        let mut pauses = 0;
        core.wait_command_progress(
            &mut || Ok(10_000_000),
            &mut |_, _| {
                polls += 1;
                Ok(0)
            },
            &mut || pauses += 1,
        )
        .unwrap();
        assert_eq!((polls, pauses), (0, 1));
        assert_eq!(core.hard_end, 500_000_000);
    }
    #[test]
    fn scope_wait_failed_or_retired_owner_does_not_wait_or_renew() {
        for state in ["fault", "finished", "cancelled"] {
            let mut core = Core::new(Rc::default(), 500_000_000);
            core.admit(0, 0, |_| Ok(fake("pending", Rc::default())))
                .unwrap();
            match state {
                "fault" => core.poison("ORIGINAL_FAULT"),
                "finished" => core.finished = true,
                _ => core.cancel_end = Some(100),
            }
            let mut polls = 0;
            let mut pauses = 0;
            assert!(core
                .wait_command_progress(
                    &mut || Ok(10),
                    &mut |_, _| {
                        polls += 1;
                        Ok(0)
                    },
                    &mut || pauses += 1
                )
                .is_err());
            assert_eq!((polls, pauses), (0, 0));
            assert_eq!(core.hard_end, 500_000_000);
            if state == "fault" {
                assert_eq!(
                    core.owner.borrow().first_fault,
                    Some(ProtocolError("ORIGINAL_FAULT"))
                );
            }
        }
    }
    #[test]
    fn scope_wait_pending_clock_failure_broadcasts_zero_without_a_pause() {
        let mut core = Core::new(Rc::default(), 500_000_000);
        let id = core
            .admit(0, 0, |_| Ok(fake("pending", Rc::default())))
            .unwrap();
        core.slots[id.index].edge.setup_pending = true;
        core.slots[id.index].edge.setup_wait_source = Some((7, 20_000_000));
        let mut polls = 0;
        let mut pauses = 0;
        assert!(core
            .wait_command_progress(
                &mut || Err(error("INJECTED_CLOCK")),
                &mut |_, _| {
                    polls += 1;
                    Ok(0)
                },
                &mut || pauses += 1
            )
            .is_err());
        assert_eq!((polls, pauses), (0, 0));
        assert_eq!(core.cancel_end, Some(0));
        assert_eq!(core.slots[id.index].completion_end, Some(0));
    }
    #[test]
    fn repository_admission_sticky_fault_never_reopens_for_a_ready_sibling() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        core.admit(1, 32, |_| Ok(fake("first", Rc::default())))
            .unwrap();
        core.poison("ORIGINAL_FAULT");
        assert!(core.repository_admission_ready().is_err());
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("ORIGINAL_FAULT"))
        );
        assert_eq!(core.slots.len(), 1);
        assert!(!core.slots[0].output_taken);
    }
    #[test]
    fn repository_admission_cancelled_and_finished_owners_refuse_without_renewal() {
        for kind in ["zero", "cancel", "finished"] {
            let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
            let slot = core
                .admit(1, 32, |_| Ok(fake("first", Rc::default())))
                .unwrap();
            if kind == "finished" {
                core.finished = true;
            } else {
                core.cancel(None, 10, if kind == "zero" { 0 } else { 100 });
            }
            let end = core.slots[slot.index].completion_end;
            assert!(core.repository_admission_ready().is_err());
            assert_eq!(core.slots[slot.index].completion_end, end);
            assert_eq!(core.slots.len(), 1);
        }
    }
    fn repository_cohort() -> (Core<Fake>, [SlotId; 2]) {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let slots = ["identity", "remotes"].map(|name| {
            let id = core
                .admit(1, 32, |_| Ok(fake(name, Rc::default())))
                .unwrap();
            core.slots[id.index].accepted_at = Some(2);
            core.slots[id.index].edge.facts = EdgeFacts {
                status: Some(ExitStatus::from_raw((id.index as i32) << 8)),
                waits_known: true,
                group_absent: true,
                terminal_eof: true,
                resource_proof: true,
                ..Default::default()
            };
            id
        });
        (core, slots)
    }
    #[test]
    fn repository_cohort_pending_sibling_consumes_no_outputs_or_authority() {
        let (mut core, slots) = repository_cohort();
        core.slots[1].accepted_at = None;
        assert!(matches!(core.take_repository_results(&slots), Ok(None)));
        assert!(core.slots.iter().all(|slot| !slot.output_taken));
        assert!(core.owner.borrow().first_fault.is_none());
        core.slots[1].accepted_at = Some(3);
        assert!(core.take_repository_results(&slots).unwrap().is_some());
    }
    #[test]
    fn repository_cohort_preserves_request_order_and_consumes_exactly_once() {
        let (mut core, slots) = repository_cohort();
        let reordered = [slots[1].clone(), slots[0].clone()];
        let results = core.take_repository_results(&reordered).unwrap().unwrap();
        assert_eq!(results.map(|r| r.status.code()), [Some(1), Some(0)]);
        assert!(core.slots.iter().all(|slot| slot.output_taken));
        assert!(core.take_repository_results(&slots).is_err());
    }
    #[test]
    fn repository_cohort_foreign_duplicate_and_consumed_slots_take_nothing() {
        for invalid in ["foreign", "duplicate", "consumed"] {
            let (mut core, mut slots) = repository_cohort();
            match invalid {
                "foreign" => slots[1].scope = Rc::new(()),
                "duplicate" => slots[1] = slots[0].clone(),
                _ => core.slots[1].output_taken = true,
            }
            assert!(core.take_repository_results(&slots).is_err());
            assert!(!core.slots[0].output_taken);
            assert_eq!(core.slots[1].output_taken, invalid == "consumed");
            assert!(core.owner.borrow().first_fault.is_some());
        }
    }
    #[test]
    fn repository_cohort_missing_proof_or_sticky_fault_never_yields_outputs() {
        for missing in ["proof", "wait", "status", "group", "terminal", "fault"] {
            let (mut core, slots) = repository_cohort();
            match missing {
                "proof" => core.slots[1].edge.facts.resource_proof = false,
                "wait" => core.slots[1].edge.facts.waits_known = false,
                "status" => core.slots[1].edge.facts.status = None,
                "group" => core.slots[1].edge.facts.group_absent = false,
                "terminal" => core.slots[1].edge.facts.terminal_eof = false,
                _ => core.poison("ORIGINAL_FAULT"),
            }
            assert!(core.take_repository_results(&slots).is_err());
            assert!(core.slots.iter().all(|slot| !slot.output_taken));
            if missing == "fault" {
                assert_eq!(
                    core.owner.borrow().first_fault,
                    Some(ProtocolError("ORIGINAL_FAULT"))
                );
            }
        }
    }
    fn installer_ready() -> Core<Fake> {
        let events = Rc::default();
        let mut core: Core<Fake> = Core::new(Rc::default(), INSTALL_OPERATION_NS);
        core.begin_installer(100_000_000, 2).unwrap();
        for name in ["a", "b"] {
            let id = core
                .admit_installer(200_000_000, 32, |_, end| {
                    assert_eq!(end, 1_100_000_000);
                    let mut edge = fake(name, Rc::clone(&events));
                    edge.facts = EdgeFacts {
                        status: Some(ExitStatus::from_raw(0)),
                        waits_known: true,
                        group_absent: true,
                        terminal_eof: true,
                        resource_proof: true,
                        io_required: true,
                        io: IoFacts {
                            stdin_closed: true,
                            stdout_eof: true,
                            stderr_eof: true,
                            ..Default::default()
                        },
                        first_fault: None,
                    };
                    Ok(edge)
                })
                .unwrap();
            core.slots[id.index].accepted_at = Some(250_000_000);
            core.take_result(&id).unwrap();
        }
        core
    }
    #[test]
    fn installer_cohort_uses_one_end_in_actual_factory_and_refuses_equality() {
        let mut core: Core<Fake> = Core::new(Rc::default(), INSTALL_OPERATION_NS);
        core.begin_installer(100_000_000, 2).unwrap();
        let parent = core.hard_end;
        let events = Rc::default();
        core.admit_installer(200_000_000, 1, |_, end| {
            assert_eq!(end, 1_100_000_000);
            Ok(fake("a", Rc::clone(&events)))
        })
        .unwrap();
        let mut called = false;
        assert!(core
            .admit_installer(1_050_000_000, 1, |_, _| {
                called = true;
                Ok(fake("b", Rc::clone(&events)))
            })
            .is_err());
        assert!(!called);
        assert_eq!(core.hard_end, parent);
        assert_eq!(core.slots[0].hard_end, 1_100_000_000);
        let mut earlier: Core<Fake> = Core::new(Rc::default(), 350_000_000);
        earlier.begin_installer(100_000_000, 1).unwrap();
        assert_eq!(earlier.installer_end().unwrap(), 350_000_000);
    }
    #[test]
    fn installer_borrowed_parent_cannot_grant_more_than_one_local_operation() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 900_000_000_000);
        core.begin_installer(100_000_000, 2).unwrap();
        assert_eq!(core.hard_end, 30_100_000_000);
        assert_eq!(core.installer_end().unwrap(), 1_100_000_000);
        assert!(core.begin_installer(500_000_000, 2).is_err());
        assert_eq!(core.hard_end, 30_100_000_000);
        let mut overflow: Core<Fake> = Core::new(Rc::default(), u64::MAX);
        assert!(overflow.begin_installer(u64::MAX - 1, 1).is_err());
        assert_eq!(overflow.cancel_end, Some(0));
    }
    #[test]
    fn installer_policy_allows_publication_after_four_seconds_without_renewal() {
        let mut core = installer_ready();
        core.seal_installer(true, &mut || Ok(300_000_000)).unwrap();
        let original = core.hard_end;
        assert!(core
            .publication_live(true, &mut || Ok(5_000_000_000))
            .is_ok());
        assert_eq!(original, 30_000_000_000);
        assert_eq!(core.hard_end, original);
        assert!(core.publication_live(true, &mut || Ok(original)).is_err());
        assert_eq!(core.hard_end, original);
    }
    #[test]
    fn installer_policy_keeps_earlier_four_second_parent_and_one_second_preflight() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 4_000_000_000);
        core.begin_installer(100_000_000, 2).unwrap();
        assert_eq!(core.hard_end, 4_000_000_000);
        assert_eq!(core.installer_end().unwrap(), 1_100_000_000);
        assert!(core.begin_installer(200_000_000, 2).is_err());
        assert_eq!(core.hard_end, 4_000_000_000);
    }
    #[test]
    fn installer_cohort_expiry_broadcasts_all_before_poll_and_keeps_first_fault() {
        let events = Rc::default();
        let mut core: Core<Fake> = Core::new(Rc::default(), INSTALL_OPERATION_NS);
        core.begin_installer(0, 2).unwrap();
        for name in ["a", "b"] {
            core.admit_installer(100_000_000, 0, |_, _| {
                let mut edge = fake(name, Rc::clone(&events));
                edge.fail_begin = name == "a";
                Ok(edge)
            })
            .unwrap();
        }
        core.poll(&mut || Ok(950_000_000));
        assert_eq!(&events.borrow()[..2], ["begin:a", "begin:b"]);
        assert!(core
            .slots
            .iter()
            .all(|slot| slot.completion_end.unwrap() <= 1_000_000_000));
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_OPERATION_DEADLINE"))
        );
        core.cancel(Some("LATER"), 970_000_000, 4_000_000_000);
        assert!(core.cancel_end.unwrap() <= 1_000_000_000);
        core.poll(&mut || Err(error("clock")));
        assert_eq!(core.cancel_end, Some(0));
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_OPERATION_DEADLINE"))
        );
    }
    #[test]
    fn installer_barrier_requires_every_physical_fact_and_retrieved_result() {
        for missing in 0..13 {
            let mut core = installer_ready();
            let slot = &mut core.slots[1];
            match missing {
                0 => slot.edge.facts.waits_known = false,
                1 => slot.edge.facts.resource_proof = false,
                2 => slot.edge.facts.group_absent = false,
                3 => slot.edge.facts.terminal_eof = false,
                4 => slot.edge.facts.io.stdin_closed = false,
                5 => slot.edge.facts.io.stdout_eof = false,
                6 => slot.edge.facts.io.stderr_eof = false,
                7 => slot.edge.facts.status = None,
                8 => slot.edge.facts.first_fault = Some("synthetic"),
                9 => slot.edge.facts.io.first_fault = Some("synthetic"),
                10 => slot.accepted_at = None,
                11 => slot.output_taken = false,
                _ => slot.accepted_at = Some(slot.hard_end),
            }
            assert!(
                core.seal_installer(true, &mut || Ok(300_000_000)).is_err(),
                "missing {missing}"
            );
            assert_eq!(core.slots.len(), 2);
            assert_eq!(core.io_reserved, 64);
            assert!(!core.finish_called);
            assert!(core.owner.borrow().first_fault.is_some());
        }
    }
    #[test]
    fn installer_barrier_checks_attached_sent_ledger_even_with_empty_local_map() {
        let (wire, _peer) = UnixStream::pair().unwrap();
        let mut endpoint = Endpoint::new(wire, "ab".repeat(16), None).unwrap();
        endpoint.send_live().unwrap();
        let root = std::process::id();
        let child = if root == 42 { 43 } else { 42 };
        endpoint
            .metadata(&format!("REGISTER|{root}|{child}|{}|0", "cd".repeat(16)))
            .unwrap();
        let mut scope = Scope::with_owner(Rc::default(), now().unwrap() + INSTALL_OPERATION_NS);
        scope.upstream = Some(endpoint);
        assert!(scope.ledger.is_empty());
        assert!(!scope.ledgers_complete());
        let mut core = installer_ready();
        assert!(core
            .seal_installer(scope.ledgers_complete(), &mut || Ok(300_000_000))
            .is_err());
    }
    #[test]
    fn installer_barrier_is_fresh_nonterminal_and_never_reopens_a_runner() {
        let mut late = installer_ready();
        assert!(late
            .seal_installer(true, &mut || Ok(1_100_000_000))
            .is_err());
        let mut broken = installer_ready();
        assert!(broken
            .seal_installer(true, &mut || Err(error("clock")))
            .is_err());
        assert_eq!(broken.cancel_end, Some(0));
        assert!(broken
            .seal_installer(true, &mut || Ok(300_000_000))
            .is_err());
        let mut core = installer_ready();
        core.seal_installer(true, &mut || Ok(300_000_000)).unwrap();
        assert_eq!(core.hard_end, INSTALL_OPERATION_NS);
        assert!(!core.finish_called && !core.finished && !core.owner.borrow().admission_closed);
        assert_eq!(core.slots.len(), 2);
        assert_eq!(core.io_reserved, 64);
        core.poll(&mut || Ok(1_500_000_000));
        core.publication_live(true, &mut || Ok(1_500_000_000))
            .unwrap();
        assert!(core
            .admit_installer(1_500_000_000, 0, |_, _| panic!("postseal spawn"))
            .is_err());
        assert!(core.begin_installer(1_500_000_000, 1).is_err());
    }
    #[test]
    fn installer_entry_refinement_never_renews_reselects_or_reopens() {
        let mut late: Core<Fake> = Core::new(Rc::default(), 4_100_000_000);
        assert!(late.refine_entry(1_100_000_000, 1_100_000_000).is_err());
        assert!(late.refine_entry(4_100_000_000, 1_100_000_000).is_err());
        assert_eq!(late.hard_end, 1_100_000_000);
        let mut earlier: Core<Fake> = Core::new(Rc::default(), 300_000_000);
        earlier.refine_entry(4_100_000_000, 100_000_000).unwrap();
        assert_eq!(earlier.hard_end, 300_000_000);
        assert!(earlier.refine_entry(4_100_000_000, 100_000_001).is_err());
        assert!(earlier.owner.borrow().admission_closed);
    }
    #[test]
    fn installer_terminal_rechecks_after_an_earlier_clean_finish() {
        let mut core = installer_ready();
        core.seal_installer(true, &mut || Ok(300_000_000)).unwrap();
        core.finish_with(&mut || Ok(INSTALL_OPERATION_NS - 60_000_000), &mut || {
            panic!("no wait")
        });
        assert!(core.finished && core.owner.borrow().first_fault.is_none());
        assert!(!core.terminal_fresh(&mut || Ok(INSTALL_OPERATION_NS)));
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_TERMINAL_DEADLINE"))
        );
        let mut clock = installer_ready();
        clock.seal_installer(true, &mut || Ok(300_000_000)).unwrap();
        clock.finish_with(&mut || Ok(400_000_000), &mut || panic!("no wait"));
        assert!(!clock.terminal_fresh(&mut || Err(error("clock"))));
        assert_eq!(clock.cancel_end, Some(0));
        assert!(!clock.terminal_fresh(&mut || Ok(400_000_001)));
    }
    #[test]
    fn normal_completion_retains_a_then_admits_b_in_same_owner() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let owner: SharedOwner = Rc::default();
        let mut core = Core::new(owner.clone(), 500_000_000);
        let a = core.admit(1, 1, |_| Ok(fake("a", events.clone()))).unwrap();
        core.request_complete(&a, 2).unwrap();
        core.poll(&mut || Ok(3));
        assert!(core.slots[0].accepted_at.is_some());
        assert!(
            !owner.borrow().admission_closed,
            "local completion closed containing owner"
        );
        let b = core.admit(4, 1, |_| Ok(fake("b", events.clone()))).unwrap();
        assert_eq!(a.index, 0);
        assert_eq!(b.index, 1);
        assert_eq!(core.slots.len(), 2);
        core.request_complete(&b, 5).unwrap();
        core.poll(&mut || Ok(6));
        assert!(core.all_resources());
        assert!(owner.borrow().first_fault.is_none());
    }
    #[test]
    fn cancellation_broadcasts_every_begin_before_poll_despite_first_begin_error() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut core = Core::new(Rc::default(), 500_000_000);
        core.admit(1, 1, |_| {
            let mut a = fake("a", events.clone());
            a.fail_begin = true;
            Ok(a)
        })
        .unwrap();
        core.admit(1, 1, |_| Ok(fake("b", events.clone()))).unwrap();
        core.cancel(Some("ORIGINAL"), 2, 500_000_000);
        core.poll(&mut || Ok(3));
        assert_eq!(
            &events.borrow()[..4],
            ["begin:a", "begin:b", "poll:a", "poll:b"]
        );
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("ORIGINAL"))
        );
        assert!(core.all_resources());
        assert!(core.admit(4, 1, |_| Ok(fake("c", events.clone()))).is_err());
    }
    #[test]
    fn direct_exit_and_group_proof_do_not_replace_either_output_eof() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut core = Core::new(Rc::default(), 500_000_000);
        let id = core
            .admit(1, 1, |_| {
                let mut a = fake("a", events);
                a.facts.io_required = true;
                Ok(a)
            })
            .unwrap();
        core.request_complete(&id, 2).unwrap();
        core.poll(&mut || Ok(3));
        assert!(core.slots[0].edge.facts().waits_known);
        assert!(!core.all_resources());
        core.slots[0].edge.facts.io.stdin_closed = true;
        core.slots[0].edge.facts.io.stdout_eof = true;
        core.poll(&mut || Ok(4));
        assert!(!core.all_resources());
        core.slots[0].edge.facts.io.stderr_eof = true;
        core.poll(&mut || Ok(5));
        assert!(core.all_resources());
    }
    #[test]
    fn proof_equality_never_accepts_and_repeated_cancel_never_extends() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut core = Core::new(Rc::default(), 500_000_000);
        let id = core.admit(1, 1, |_| Ok(fake("a", events))).unwrap();
        core.request_complete(&id, 2).unwrap();
        let end = core.slots[0].completion_end.unwrap();
        core.poll(&mut || Ok(end));
        assert!(!core.all_resources());
        assert!(core.owner.borrow().first_fault.is_some());
        let bound = core.cancel_end;
        core.cancel(None, 100_000_000, 500_000_000);
        assert_eq!(core.cancel_end, bound);
    }
    #[test]
    fn slot_identity_and_whole_scope_buffer_budget_are_not_recycled() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut a = Core::new(Rc::default(), 500_000_000);
        let mut b: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let id = a
            .admit(1, MAX_IO_BYTES, |_| Ok(fake("a", events.clone())))
            .unwrap();
        assert!(b.index(&id).is_err());
        a.request_complete(&id, 2).unwrap();
        a.poll(&mut || Ok(3));
        assert!(a.admit(4, 1, |_| Ok(fake("b", events.clone()))).is_err());
        assert_eq!(a.slots.len(), 1);
        assert_eq!(a.io_reserved, MAX_IO_BYTES);
    }
    #[test]
    fn a_valid_zero_clock_is_not_replaced_with_a_clock_failure() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut core = Core::new(Rc::default(), 500_000_000);
        core.admit(0, 1, |_| {
            let mut a = fake("a", events);
            a.facts.status = Some(ExitStatus::from_raw(0));
            Ok(a)
        })
        .unwrap();
        core.poll(&mut || Ok(0));
        core.poll(&mut || Ok(0));
        assert_eq!(core.slots[0].accepted_at, Some(0));
        assert!(core.owner.borrow().first_fault.is_none());
    }
    #[test]
    fn final_finish_requires_a_fresh_strictly_before_bound_clock_even_if_empty() {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        core.cancel(None, 2, 500_000_000);
        assert!(core.all_resources());
        assert!(core.finish_at(core.cancel_end.unwrap()));
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_CLEANUP_DEADLINE"))
        );
    }
    #[test]
    fn result_is_single_use_and_failed_operation_never_returns_output() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut core = Core::new(Rc::default(), 500_000_000);
        let id = core.admit(1, 1, |_| Ok(fake("a", events))).unwrap();
        core.request_complete(&id, 2).unwrap();
        core.poll(&mut || Ok(3));
        assert!(core.take_result(&id).unwrap().status.success());
        assert!(core.take_result(&id).is_err());
        assert_eq!(core.slots.len(), 1);
        assert!(core.slots[0].accepted_at.is_some());
        assert_eq!(
            core.owner.borrow().first_fault,
            Some(ProtocolError("SCOPE_RESULT"))
        );
    }
    #[test]
    fn r38_final_finish_clock_error_irreversibly_zeroes_all_pending_ends() {
        for (empty, prior_fault) in [(false, false), (false, true), (true, false)] {
            let events: Rc<RefCell<Vec<String>>> = Rc::default();
            let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
            if !empty {
                let accepted = core
                    .admit(1, 0, |_| Ok(fake("accepted", events.clone())))
                    .unwrap();
                core.request_complete(&accepted, 2).unwrap();
                core.poll(&mut || Ok(3));
                assert!(core.slots[0].accepted_at.is_some());
                core.admit(4, 0, |_| {
                    let mut pending = fake("pending", events.clone());
                    pending.facts.io_required = true;
                    Ok(pending)
                })
                .unwrap();
            }
            if prior_fault {
                core.poison("ORIGINAL");
            }
            events.borrow_mut().clear();
            let mut calls = 0;
            core.finish_with(
                &mut || {
                    calls += 1;
                    match calls {
                        1 => Ok(100),
                        2 => Ok(101),
                        3 => Err(error("SCOPE_CLOCK")),
                        _ => Ok(200),
                    }
                },
                &mut || panic!("final failed clock must end this envelope"),
            );
            assert_eq!(calls, 3);
            assert_eq!(
                core.cancel_end,
                Some(0),
                "final clock failure retained a positive end"
            );
            assert_eq!(
                core.owner.borrow().first_fault,
                Some(ProtocolError(if prior_fault {
                    "ORIGINAL"
                } else {
                    "SCOPE_CLOCK"
                }))
            );
            assert!(core.finished);
            assert!(core.finish_called);
            if !empty {
                assert!(core.slots[0].accepted_at.is_some());
                assert!(core.slots[1].accepted_at.is_none());
                assert_eq!(core.slots[1].completion_end, Some(0));
                assert_eq!(core.slots[1].edge.begin_ends, [50_000_100, 0]);
                assert_eq!(
                    &*events.borrow(),
                    &["begin:pending", "poll:pending", "begin:pending"]
                );
            }
            let before = events.borrow().clone();
            core.cancel(None, 200, 500_000_000);
            core.finish_with(&mut || Ok(201), &mut || {
                panic!("finished failure cannot restart")
            });
            assert_eq!(core.cancel_end, Some(0));
            assert_eq!(*events.borrow(), before);
        }
    }
    #[test]
    fn r38_ninth_slot_is_polled_on_next_round_robin_turn() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut core = Core::new(Rc::default(), 500_000_000);
        for name in ["0", "1", "2", "3", "4", "5", "6", "7", "8"] {
            core.admit(1, 0, |_| Ok(fake(name, events.clone())))
                .unwrap();
        }
        core.poll(&mut || Ok(2));
        assert_eq!(
            &*events.borrow(),
            &["poll:0", "poll:1", "poll:2", "poll:3", "poll:4", "poll:5", "poll:6", "poll:7",]
        );
        core.poll(&mut || Ok(3));
        assert_eq!(events.borrow()[8], "poll:8");
        assert_eq!(events.borrow().len(), 16);
        assert_eq!(core.slots.len(), 9);
    }
    #[test]
    fn r40_leaf_environment_preserves_routing_but_removes_parent_control() {
        let mut command = Command::new("inert-never-execute");
        command
            .arg("--version")
            .env("OPENAI_BASE_URL", "https://proxy.invalid")
            .env("CODEX_HOME", "/private/owned-profile")
            .env("KEITO_CAPTURE_ACK_FD", "3")
            .env("KEITO_PROCESS_OWNER_NONCE", "parent");
        clear_parent_environment(&mut command);
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env[std::ffi::OsStr::new("OPENAI_BASE_URL")],
            Some(std::ffi::OsStr::new("https://proxy.invalid"))
        );
        assert_eq!(
            env[std::ffi::OsStr::new("CODEX_HOME")],
            Some(std::ffi::OsStr::new("/private/owned-profile"))
        );
        assert_eq!(env[std::ffi::OsStr::new("KEITO_CAPTURE_ACK_FD")], None);
        assert_eq!(env[std::ffi::OsStr::new("KEITO_PROCESS_OWNER_NONCE")], None);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new("--version")]
        );
    }
    #[test]
    #[cfg(feature = "capture-profile")]
    fn repository_wait_profile_classifies_retained_stages_without_progress_or_proof() {
        use crate::capture_profile::Phase;
        for (first, second, expected) in [
            (
                Some(Phase::RepoRunningPoll),
                Some(Phase::RepoRunningPoll),
                Phase::RepoRunningWait,
            ),
            (
                Some(Phase::RepoCleaningPoll),
                Some(Phase::RepoDonePoll),
                Phase::RepoCleaningWait,
            ),
            (
                Some(Phase::RepoRunningPoll),
                Some(Phase::RepoCleaningPoll),
                Phase::RepoMixedWait,
            ),
            (
                Some(Phase::RepoParkPoll),
                Some(Phase::RepoRunningPoll),
                Phase::RepoMixedWait,
            ),
            (None, None, Phase::RepoUnknownWait),
            (None, Some(Phase::RepoRunningPoll), Phase::RepoMixedWait),
        ] {
            let events = Rc::new(RefCell::new(Vec::new()));
            let mut core = Core::new(Rc::default(), 500_000_000);
            let a = core.admit(1, 0, |_| Ok(fake("a", events.clone()))).unwrap();
            let b = core.admit(2, 0, |_| Ok(fake("b", events.clone()))).unwrap();
            core.slots[a.index].edge.repository_stage = first;
            core.slots[b.index].edge.repository_stage = second;
            let cursor = core.cursor;
            assert_eq!(core.repository_wait_phase() as usize, expected as usize);
            assert_eq!(core.cursor, cursor);
            assert_eq!(core.hard_end, 500_000_000);
            assert!(core.cancel_end.is_none());
            assert!(events.borrow().is_empty());
            assert!(core.slots.iter().all(|slot| slot.accepted_at.is_none()
                && !slot.output_taken
                && !slot.edge.facts().resources_complete()));
        }
    }
    #[test]
    #[cfg(feature = "capture-profile")]
    fn repository_wait_profile_ignores_accepted_slots_without_consuming_them() {
        use crate::capture_profile::Phase;
        let mut core = Core::new(Rc::default(), 500_000_000);
        assert_eq!(
            core.repository_wait_phase() as usize,
            Phase::RepoUnknownWait as usize
        );
        let a = core.admit(1, 0, |_| Ok(fake("a", Rc::default()))).unwrap();
        let b = core.admit(2, 0, |_| Ok(fake("b", Rc::default()))).unwrap();
        core.slots[a.index].edge.repository_stage = Some(Phase::RepoRunningPoll);
        core.slots[a.index].accepted_at = Some(3);
        core.slots[b.index].edge.repository_stage = Some(Phase::RepoDonePoll);
        assert_eq!(
            core.repository_wait_phase() as usize,
            Phase::RepoCleaningWait as usize
        );
        assert_eq!(core.slots[a.index].accepted_at, Some(3));
        assert!(!core.slots[a.index].output_taken);
        assert!(core.slots[b.index].accepted_at.is_none());
    }
    #[test]
    #[cfg(feature = "capture-profile")]
    fn repository_wait_profile_keeps_one_original_pause_and_withholds_unproved_output() {
        use crate::capture_profile::Phase;
        for stage in [
            Phase::RepoRunningPoll,
            Phase::RepoCleaningPoll,
            Phase::RepoDonePoll,
        ] {
            let mut core = Core::new(Rc::default(), 500_000_000);
            let slot = core
                .admit(1, 0, |_| Ok(fake("retained", Rc::default())))
                .unwrap();
            core.slots[slot.index].edge.repository_stage = Some(stage);
            let mut pauses = 0;
            core.wait_command_progress(
                &mut || Ok(10_000_000),
                &mut |_, _| panic!("original sleep path became fd poll"),
                &mut || pauses += 1,
            )
            .unwrap();
            assert_eq!(pauses, 1);
            assert!(core.slots[slot.index].accepted_at.is_none());
            assert!(!core.slots[slot.index].output_taken);
            assert!(!core.slots[slot.index].edge.facts().resources_complete());
            assert_eq!(core.hard_end, 500_000_000);
            assert!(core.cancel_end.is_none());
        }
    }
    fn running_wait_core(events: Rc<RefCell<Vec<String>>>) -> (Core<Fake>, SlotId) {
        let mut core = Core::new(Rc::default(), 500_000_000);
        let id = core.admit(1, 0, |_| Ok(fake("waiting", events))).unwrap();
        core.slots[id.index].edge.progress_sources = vec![(7, libc::POLLIN)];
        core.slots[id.index].edge.progress_end = Some(400_000_000);
        (core, id)
    }
    #[test]
    fn running_wait_retained_socket_readiness_schedules_original_turn_before_blind_pause() {
        use std::cell::Cell;
        // Old blind branch advances1ms, missing the original200us boundary.
        let at = Cell::new(100_000_000u64);
        let events = Rc::new(RefCell::new(Vec::new()));
        let (mut core, id) = running_wait_core(events.clone());
        let pauses = Cell::new(0);
        core.wait_command_progress(
            &mut || Ok(at.get()),
            &mut |fds, timeout| {
                assert_eq!(timeout, 1);
                assert_eq!(fds[0].fd, 7);
                at.set(100_100_000);
                fds[0].revents = libc::POLLIN | libc::POLLHUP;
                Ok(1)
            },
            &mut || {
                pauses.set(pauses.get() + 1);
                at.set(at.get() + 1_000_000);
            },
        )
        .unwrap();
        assert_eq!(pauses.get(), 0);
        assert!(
            at.get() < 100_200_000,
            "blind pause postponed original GONE/EOF parser turn"
        );
        assert!(events.borrow().is_empty());
        core.poll(&mut || Ok(at.get()));
        assert_eq!(events.borrow().as_slice(), ["poll:waiting"]);
        assert!(core.slots[id.index].accepted_at.is_none());
        assert!(!core.slots[id.index].output_taken);
        assert!(!core.slots[id.index].edge.facts().resources_complete());
    }
    #[test]
    fn running_wait_caps_to_earliest_original_end_and_floors_final_fraction() {
        for (slot_end, completion_end, edge_end, at, expected) in [
            (500_000_000, None, 400_000_000, 100_000_000, 1),
            (100_900_000, None, 400_000_000, 100_000_000, 0),
            (500_000_000, Some(100_700_000), 400_000_000, 100_000_000, 0),
            (500_000_000, None, 100_999_999, 100_000_000, 0),
            (500_000_000, None, 500_000_000, 449_500_000, 0),
        ] {
            let (mut core, id) = running_wait_core(Rc::default());
            core.slots[id.index].hard_end = slot_end;
            core.slots[id.index].completion_end = completion_end;
            core.slots[id.index].edge.progress_end = Some(edge_end);
            let mut polls = 0;
            core.wait_command_progress(
                &mut || Ok(at),
                &mut |_, timeout| {
                    polls += 1;
                    assert_eq!(timeout, expected);
                    Ok(0)
                },
                &mut || panic!("useful retainedFD became blind pause"),
            )
            .unwrap();
            assert_eq!(polls, 1);
            assert_eq!(core.slots[id.index].hard_end, slot_end);
            assert_eq!(core.slots[id.index].completion_end, completion_end);
            assert!(core.slots[id.index].accepted_at.is_none());
        }
    }
    #[test]
    fn running_wait_buffered_frame_schedules_parser_but_partial_without_fd_keeps_pause() {
        for (buffered, expected_pauses) in [(true, 0), (false, 1)] {
            let (mut core, id) = running_wait_core(Rc::default());
            // No usefulFD after actual EOF, or partial bytes without anFD.
            core.slots[id.index].edge.progress_sources.clear();
            core.slots[id.index].edge.progress_buffered = buffered;
            let mut pauses = 0;
            core.wait_command_progress(
                &mut || Ok(100_000_000),
                &mut |_, _| panic!("no useful source was polled"),
                &mut || pauses += 1,
            )
            .unwrap();
            assert_eq!(pauses, expected_pauses);
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].edge.facts().waits_known);
        }
    }
    #[test]
    fn running_wait_no_fd_final_fraction_does_not_sleep_past_earliest_end() {
        let (mut core, id) = running_wait_core(Rc::default());
        core.slots[id.index].edge.progress_sources.clear();
        core.slots[id.index].completion_end = Some(100_999_999);
        core.wait_command_progress(
            &mut || Ok(100_000_000),
            &mut |_, _| panic!("absentFD poll"),
            &mut || panic!("final fraction rounded up"),
        )
        .unwrap();
        assert!(core.slots[id.index].accepted_at.is_none());
    }
    #[test]
    fn running_wait_invalid_fd_clock_and_expiry_refuse_before_poll() {
        for kind in ["invalid", "clock", "completion", "operation"] {
            let (mut core, id) = running_wait_core(Rc::default());
            if kind == "invalid" {
                core.slots[id.index].edge.progress_sources[0].0 = -1;
            }
            if kind == "completion" {
                core.slots[id.index].completion_end = Some(100_000_000);
            }
            let mut polls = 0;
            let mut pauses = 0;
            let result = core.wait_command_progress(
                &mut || {
                    if kind == "clock" {
                        Err(error("INJECTED_CLOCK"))
                    } else if kind == "operation" {
                        Ok(450_000_000)
                    } else {
                        Ok(100_000_000)
                    }
                },
                &mut |_, _| {
                    polls += 1;
                    Ok(0)
                },
                &mut || pauses += 1,
            );
            assert!(result.is_err());
            assert_eq!((polls, pauses), (0, 0));
            assert!(core.owner.borrow().first_fault.is_some());
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].output_taken);
            if kind == "clock" {
                assert_eq!(core.cancel_end, Some(0));
                assert_eq!(core.slots[id.index].completion_end, Some(0));
            }
        }
    }
    #[test]
    fn running_wait_eintr_hup_timeout_are_not_proof_and_poll_faults_stick() {
        for kind in ["eintr", "hup", "timeout", "nval", "negative", "error"] {
            let (mut core, id) = running_wait_core(Rc::default());
            let result = core.wait_command_progress(
                &mut || Ok(100_000_000),
                &mut |fds, _| match kind {
                    "eintr" => Err(io::Error::from(io::ErrorKind::Interrupted)),
                    "hup" => {
                        fds[0].revents = libc::POLLHUP;
                        Ok(1)
                    }
                    "nval" => {
                        fds[0].revents = libc::POLLNVAL;
                        Ok(1)
                    }
                    "negative" => Ok(-1),
                    "error" => Err(io::Error::from(io::ErrorKind::Other)),
                    _ => Ok(0),
                },
                &mut || panic!("unexpected blind pause"),
            );
            assert_eq!(
                result.is_err(),
                matches!(kind, "nval" | "negative" | "error")
            );
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].edge.facts().waits_known);
            assert!(!core.slots[id.index].edge.facts().terminal_eof);
            assert!(!core.slots[id.index].output_taken);
        }
    }
    #[test]
    fn running_wait_rechecks_original_end_and_owner_after_wakeup() {
        use std::cell::Cell;
        for fault in [false, true] {
            let (mut core, id) = running_wait_core(Rc::default());
            core.slots[id.index].completion_end = Some(100_500_000);
            let owner = core.owner.clone();
            let at = Cell::new(100_000_000);
            let result = core.wait_command_progress(
                &mut || Ok(at.get()),
                &mut |_, _| {
                    if fault {
                        owner.borrow_mut().poison(ProtocolError("ORIGINAL_FAULT"));
                    } else {
                        at.set(100_500_000);
                    }
                    Ok(1)
                },
                &mut || panic!("unexpected blind pause"),
            );
            assert!(result.is_err());
            assert!(core.slots[id.index].accepted_at.is_none());
            assert!(!core.slots[id.index].output_taken);
            if fault {
                assert_eq!(
                    core.owner.borrow().first_fault,
                    Some(ProtocolError("ORIGINAL_FAULT"))
                );
            } else {
                assert_eq!(core.cancel_end, Some(150_500_000));
                assert_eq!(core.slots[id.index].completion_end, Some(100_500_000));
            }
        }
    }
    #[test]
    fn running_wait_local_expiry_preserves_sibling_cleanup_envelope() {
        use std::cell::Cell;
        for expired_before_wait in [true, false] {
            let (mut core, expired) = running_wait_core(Rc::default());
            let sibling = core
                .admit(2, 0, |_| Ok(fake("sibling", Rc::default())))
                .unwrap();
            core.request_complete(&expired, 100_000_000).unwrap();
            core.slots[expired.index].edge.progress_end = Some(150_000_000);
            let at = Cell::new(if expired_before_wait {
                150_000_000
            } else {
                149_000_000
            });
            let mut polls = 0;
            let result = core.wait_command_progress(
                &mut || Ok(at.get()),
                &mut |_, _| {
                    polls += 1;
                    at.set(150_000_000);
                    Ok(0)
                },
                &mut || panic!("retained source became blind pause"),
            );
            assert!(result.is_err());
            assert_eq!(polls, usize::from(!expired_before_wait));
            assert_eq!(core.cancel_end, Some(200_000_000));
            assert_eq!(core.slots[expired.index].completion_end, Some(150_000_000));
            assert_eq!(core.slots[sibling.index].completion_end, Some(200_000_000));
            assert_eq!(
                core.slots[expired.index].edge.begin_ends,
                [150_000_000, 150_000_000]
            );
            assert_eq!(core.slots[sibling.index].edge.begin_ends, [200_000_000]);
            assert!(core
                .slots
                .iter()
                .all(|slot| slot.accepted_at.is_none() && !slot.output_taken));
        }
    }
    #[test]
    fn running_wait_does_not_reset_cursor_or_release_a_partial_repository_cohort() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut core = Core::new(Rc::default(), 500_000_000);
        let mut ids = Vec::new();
        for _ in 0..9 {
            ids.push(
                core.admit(1, 0, |_| Ok(fake("slot", events.clone())))
                    .unwrap(),
            );
        }
        core.cursor = 8;
        core.slots[8].edge.progress_sources = vec![(7, libc::POLLIN)];
        core.wait_command_progress(&mut || Ok(100_000_000), &mut |_, _| Ok(1), &mut || {
            panic!("blind pause")
        })
        .unwrap();
        assert_eq!(core.cursor, 8);
        assert!(events.borrow().is_empty());
        core.poll(&mut || Ok(100_000_000));
        assert_eq!(events.borrow().len(), SLOTS_PER_TURN);
        assert_eq!(core.cursor, 7);
        assert!(core
            .take_repository_results(&[ids[0].clone(), ids[8].clone()])
            .unwrap()
            .is_none());
        assert!(core
            .slots
            .iter()
            .all(|slot| slot.accepted_at.is_none() && !slot.output_taken));
    }
    #[test]
    fn running_wait_upstream_budget_defers_wait_for_original_cancel_pump() {
        use super::super::protocol::{PollWork, MAX_PARSE, MAX_READS};
        for (reads, parsed, expected) in [
            (MAX_READS, 0, 0),
            (0, MAX_PARSE, 0),
            (MAX_READS - 1, MAX_PARSE - 1, 1),
        ] {
            let work = PollWork { reads, parsed };
            let mut waits = 0;
            wait_with_upstream_budget(Some(&work), || {
                waits += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(waits, expected);
        }
        let mut waits = 0;
        wait_with_upstream_budget(None, || {
            waits += 1;
            Err(error("ORIGINAL_WAIT"))
        })
        .unwrap_err();
        assert_eq!(waits, 1);
    }

    #[test]
    fn running_wait_ready_sibling_does_not_release_previously_accepted_cohort_output() {
        let (mut core, pending) = running_wait_core(Rc::default());
        let accepted = core
            .admit(2, 0, |_| Ok(fake("accepted", Rc::default())))
            .unwrap();
        core.slots[accepted.index].accepted_at = Some(3);
        core.slots[accepted.index].edge.facts = EdgeFacts {
            status: Some(ExitStatus::from_raw(0)),
            waits_known: true,
            group_absent: true,
            terminal_eof: true,
            resource_proof: true,
            ..Default::default()
        };
        core.wait_command_progress(
            &mut || Ok(100_000_000),
            &mut |fds, _| {
                fds[0].revents = libc::POLLIN;
                Ok(1)
            },
            &mut || panic!("blind pause"),
        )
        .unwrap();
        assert!(core
            .take_repository_results(&[accepted.clone(), pending.clone()])
            .unwrap()
            .is_none());
        assert_eq!(core.slots[accepted.index].accepted_at, Some(3));
        assert!(!core.slots[accepted.index].output_taken);
        assert!(core.slots[pending.index].accepted_at.is_none());
        assert!(!core.slots[pending.index].output_taken);
    }
    #[test]
    fn running_wait_clock_failure_after_wakeup_broadcasts_zero_without_proof() {
        use std::cell::Cell;
        let (mut core, id) = running_wait_core(Rc::default());
        let reads = Cell::new(0);
        assert!(core
            .wait_command_progress(
                &mut || {
                    reads.set(reads.get() + 1);
                    if reads.get() == 1 {
                        Ok(100_000_000)
                    } else {
                        Err(error("INJECTED_CLOCK"))
                    }
                },
                &mut |_, _| Ok(1),
                &mut || panic!("blind pause")
            )
            .is_err());
        assert_eq!(core.cancel_end, Some(0));
        assert_eq!(core.slots[id.index].completion_end, Some(0));
        assert!(core.slots[id.index].accepted_at.is_none());
        assert!(!core.slots[id.index].output_taken);
    }
}
