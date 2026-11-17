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
    fn setup_pending(&self) -> bool {
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
    fn setup_pending(&self) -> bool {
        matches!(self, Self::Ready | Self::Park)
    }
}
struct GroupEdge {
    lease: GroupLease,
    command: Option<Command>,
    pipes: PipeIo,
    stage: Stage,
    proof: bool,
    first_fault: Option<&'static str>,
    published: [bool; 4],
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
                    let attached = if self.lease.report().workload_spawned {
                        self.pipes.attach(
                            self.lease.stdin(),
                            self.lease.stdout(),
                            self.lease.stderr(),
                        )
                    } else {
                        Ok(())
                    };
                    self.stage = Stage::Park;
                    started?;
                    attached?;
                }
            }
            Stage::Park => {
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
        Ok(())
    }
}
impl Edge for GroupEdge {
    #[cfg(feature = "dev-tools")]
    fn setup_trace(&self) -> Option<super::group::SetupTrace> {
        Some(self.lease.setup_trace())
    }
    fn setup_pending(&self) -> bool {
        self.stage.setup_pending()
    }
    fn begin(&mut self, end: u64) -> io::Result<()> {
        self.command.take();
        self.pipes.cancel_input();
        if matches!(self.stage, Stage::Done) {
            return Ok(());
        }
        self.stage = Stage::Cleaning;
        self.lease.begin_local_cleanup(end)
    }
    fn poll(&mut self) -> io::Result<()> {
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
            if self.slots[index].accepted_at.is_some() {
                continue;
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
    // Validate the entire fixed cohort BEFORE consuming any result. An earlier
    // accepted sibling cannot leak output while another is pending or invalid.
    fn take_repository_results(
        &mut self,
        slots: &[SlotId; 3],
    ) -> io::Result<Option<[CommandResult; 3]>> {
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
            self.take_result(&slots[2])?,
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

fn await_repository_admission<E: Edge>(
    core: &mut Core<E>,
    progress: &mut impl FnMut(&mut Core<E>),
    pause: &mut impl FnMut(std::time::Duration),
) -> io::Result<()> {
    loop {
        progress(core);
        if core.repository_admission_ready()? {
            return Ok(());
        }
        // A fixed sleep can skip the remaining READY/PARK window. Yield the
        // entry thread, then repeat the SAME bounded progress/fault checks.
        pause(std::time::Duration::ZERO);
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
        self.start_cooperative_inner(&mut command, None)
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
        self.start_cooperative_inner(&mut command, Some(plan))
    }
    fn start_cooperative_inner(
        &mut self,
        command: &mut Command,
        plan: Option<IoPlan>,
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
        let admission_end = self.core.hard_end.saturating_sub(RESERVE_NS);
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
            if fresh >= admission_end || super::signals::pending() {
                return Err(error("SCOPE_ADMISSION"));
            }
            let child = command.spawn()?;
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
        let hard_end = if installer {
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
            }))
        };
        let result = if installer {
            self.core.admit_installer(at, capacity, create_edge)
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
    /// Commands are the original three Git values; no shell or worker thread.
    pub(crate) fn run_verified_repository(
        &mut self,
        commands: [Command; 3],
        image: &super::image::VerifiedKeeperImage,
    ) -> io::Result<[CommandResult; 3]> {
        let result = (|| {
            // All IO plans exist before the first child. Actual allocation and
            // capacity reservation still precede each registered edge's spawn.
            let plans = [
                IoPlan::new(Vec::new(), 8192, 8192)?,
                IoPlan::new(Vec::new(), 8192, 8192)?,
                IoPlan::new(Vec::new(), 1024 * 1024, 8192)?,
            ];
            let mut slots = [None, None, None];
            for ((command, plan), slot) in commands.into_iter().zip(plans).zip(&mut slots) {
                // The private fixed cohort has at most three edges here.
                // A blocking sibling spawn must not consume an earlier
                // keeper's already-running short setup window.
                let upstream = &mut self.upstream;
                let ledger = &mut self.ledger;
                await_repository_admission(
                    &mut self.core,
                    &mut |core| Self::poll_turn(core, upstream, ledger),
                    &mut |duration| {
                        if duration.is_zero() {
                            std::thread::yield_now();
                        } else {
                            std::thread::sleep(duration);
                        }
                    },
                )?;
                *slot = Some(self.start_verified_opaque(command, plan, image)?);
                self.poll();
                if let Some(reason) = self.core.owner.borrow().first_fault {
                    return Err(error(reason.0));
                }
            }
            let slots = slots.map(|slot| slot.expect("three admitted repository slots"));
            loop {
                self.poll();
                if let Some(results) = self.core.take_repository_results(&slots)? {
                    return Ok(results);
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
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
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
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
        Self::pump(core, upstream, ledger, start);
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
        events: Rc<RefCell<Vec<String>>>,
        name: &'static str,
        facts: EdgeFacts,
        begun: bool,
        fail_begin: bool,
        begin_ends: Vec<u64>,
        setup_pending: bool,
        #[cfg(feature = "dev-tools")]
        setup_trace: Option<super::super::group::SetupTrace>,
    }
    impl Edge for Fake {
        #[cfg(feature = "dev-tools")]
        fn setup_trace(&self) -> Option<super::super::group::SetupTrace> {
            self.setup_trace
        }
        fn setup_pending(&self) -> bool {
            self.setup_pending
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
            name,
            events,
            facts: EdgeFacts::default(),
            begun: false,
            fail_begin: false,
            begin_ends: vec![],
            setup_pending: false,
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
            &mut |duration| {
                // A yield permits one bounded microsecond of peer progress;
                // a deliberate millisecond sleep skips this original window.
                clock.set(clock.get() + (duration.as_nanos() as u64).max(1_000));
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
            &mut |_| pauses += 1,
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
    fn repository_cohort() -> (Core<Fake>, [SlotId; 3]) {
        let mut core: Core<Fake> = Core::new(Rc::default(), 500_000_000);
        let slots = ["root", "common", "remotes"].map(|name| {
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
        core.slots[2].accepted_at = None;
        assert!(matches!(core.take_repository_results(&slots), Ok(None)));
        assert!(core.slots.iter().all(|slot| !slot.output_taken));
        assert!(core.owner.borrow().first_fault.is_none());
        core.slots[2].accepted_at = Some(3);
        assert!(core.take_repository_results(&slots).unwrap().is_some());
    }
    #[test]
    fn repository_cohort_preserves_request_order_and_consumes_exactly_once() {
        let (mut core, slots) = repository_cohort();
        let reordered = [slots[2].clone(), slots[0].clone(), slots[1].clone()];
        let results = core.take_repository_results(&reordered).unwrap().unwrap();
        assert_eq!(
            results.map(|r| r.status.code()),
            [Some(2), Some(0), Some(1)]
        );
        assert!(core.slots.iter().all(|slot| slot.output_taken));
        assert!(core.take_repository_results(&slots).is_err());
    }
    #[test]
    fn repository_cohort_foreign_duplicate_and_consumed_slots_take_nothing() {
        for invalid in ["foreign", "duplicate", "consumed"] {
            let (mut core, mut slots) = repository_cohort();
            match invalid {
                "foreign" => slots[2].scope = Rc::new(()),
                "duplicate" => slots[2] = slots[1].clone(),
                _ => core.slots[2].output_taken = true,
            }
            assert!(core.take_repository_results(&slots).is_err());
            assert!(!core.slots[0].output_taken && !core.slots[1].output_taken);
            assert!(core.owner.borrow().first_fault.is_some());
        }
    }
    #[test]
    fn repository_cohort_missing_proof_or_sticky_fault_never_yields_outputs() {
        for missing in ["proof", "wait", "status", "group", "terminal", "fault"] {
            let (mut core, slots) = repository_cohort();
            match missing {
                "proof" => core.slots[2].edge.facts.resource_proof = false,
                "wait" => core.slots[2].edge.facts.waits_known = false,
                "status" => core.slots[2].edge.facts.status = None,
                "group" => core.slots[2].edge.facts.group_absent = false,
                "terminal" => core.slots[2].edge.facts.terminal_eof = false,
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
}
