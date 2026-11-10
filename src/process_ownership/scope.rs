//! Explicit entry-thread command owner. No implicit/TLS runner or positive
//! capture authority. Cooperative edges and sealed product images remain pending.
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
            if !self.published[index] && pid.is_some() && (index < 2 || status.is_some()) {
                self.published[index] = true;
                let kind = if index < 2 { "REGISTER" } else { "WAITED" };
                let value = status.map(ExitStatus::into_raw).unwrap_or(0);
                events.push(format!("{kind}|{parent}|{}|{nonce}|{value}", pid.unwrap()));
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
    cancel_sent: bool,
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
        if !self.cancel_sent && self.status.is_none() && !self.endpoint.is_settled() {
            self.cancel_sent = true;
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
    completion_end: Option<u64>,
    accepted_at: Option<u64>,
    output_taken: bool,
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
        }
    }
    fn poison(&mut self, reason: &'static str) {
        self.owner.borrow_mut().poison(ProtocolError(reason));
    }
    fn admission_clock(&mut self, clock: &mut impl FnMut() -> io::Result<u64>) -> io::Result<u64> {
        clock()
    }
    fn admit(
        &mut self,
        at: u64,
        capacity: usize,
        create: impl FnOnce(SharedOwner) -> io::Result<E>,
    ) -> io::Result<SlotId> {
        let admission = self.owner.borrow().check_admission();
        if let Err(reason) = admission {
            return Err(error(reason.0));
        }
        if self.finished
            || self.cancel_end.is_some()
            || at >= self.hard_end.saturating_sub(RESERVE_NS)
        {
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
        let edge = match create(self.owner.clone()) {
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
            completion_end: None,
            accepted_at: None,
            output_taken: false,
        });
        self.io_reserved = reserved.unwrap();
        Ok(id)
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
        let end = self.hard_end.min(at.saturating_add(RESERVE_NS));
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
        if at >= self.hard_end.saturating_sub(RESERVE_NS) && self.cancel_end.is_none() {
            self.cancel(Some("SCOPE_OPERATION_DEADLINE"), at, self.hard_end);
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
}
impl Scope {
    /// Called on the entry thread after image bootstrap and before CLI parsing
    /// or runtime construction. Absence and malformed metadata are distinct.
    pub fn entry_from_environment(local_ns: u64) -> io::Result<Option<Self>> {
        super::signals::install()?;
        let at = now()?;
        let end = at
            .checked_add(local_ns)
            .ok_or_else(|| error("SCOPE_CLOCK"))?;
        let Some((attachment, stream)) = super::attachment::consume(at, end)? else {
            return Ok(None);
        };
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
        }))
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
    pub fn with_owner(owner: SharedOwner, hard_end: u64) -> Self {
        Self {
            core: Core::new(owner, hard_end),
            upstream: None,
            ledger: BTreeMap::new(),
            terminal_sent: false,
        }
    }
    fn pump(
        core: &mut Core<ProductEdge>,
        upstream: &mut Option<Endpoint>,
        ledger: &mut BTreeMap<Registration, bool>,
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
        for slot in &mut core.slots {
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
        let result = self.core.admit(at, capacity, |owner| {
            let endpoint =
                Endpoint::with_owner(parent, nonce.clone(), None, owner).map_err(|e| error(e.0))?;
            let pipes = plan.map(IoPlan::prepare).transpose()?;
            if now()? >= admission_end || super::signals::pending() {
                return Err(error("SCOPE_ADMISSION"));
            }
            let child = command.spawn()?;
            // The edge owns the actual handle before identity publication.
            let mut edge = CooperativeEdge {
                child,
                endpoint,
                nonce,
                status: None,
                cancel_sent: false,
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
        self.poll();
        if result.is_err() {
            self.begin_cancel(ProtocolError("SCOPE_COOPERATIVE_SPAWN"), self.core.hard_end);
        }
        result
    }
    /// QA-owned700 image constructor only. A sealed official-pair image entry
    /// point is still required before wiring installed product call sites.
    pub fn start_owned_opaque(
        &mut self,
        command: Command,
        plan: IoPlan,
        root: &Path,
        image: &Path,
    ) -> io::Result<SlotId> {
        self.start_with_lease(command, plan, |owner, end| {
            GroupLease::spawn_keeper_with_policy(owner, root, image, end, CleanupPolicy::HookShort)
        })
    }
    pub(crate) fn start_verified_opaque(
        &mut self,
        command: Command,
        plan: IoPlan,
        image: &super::image::VerifiedKeeperImage,
    ) -> io::Result<SlotId> {
        self.start_with_lease(command, plan, |owner, end| {
            GroupLease::spawn_verified_keeper(owner, image, end)
        })
    }
    fn start_with_lease(
        &mut self,
        mut command: Command,
        plan: IoPlan,
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
        let hard_end = self.core.hard_end;
        let result = self.core.admit(at, capacity, |owner| {
            // Scope capacity/slot admission precedes buffer allocation, and
            // all buffers exist before spawning the first owned handle.
            let pipes = match plan.prepare() {
                Ok(pipes) => pipes,
                Err(_) => {
                    owner.borrow_mut().poison(ProtocolError("SCOPE_IO_PREPARE"));
                    return Err(error("SCOPE_IO_PREPARE"));
                }
            };
            let lease = create(owner, hard_end)?;
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
        let slot = match self.start_verified_opaque(command, plan, image) {
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
        Self::pump(&mut self.core, &mut self.upstream, &mut self.ledger);
        self.core.poll(&mut now);
        Self::pump(&mut self.core, &mut self.upstream, &mut self.ledger);
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
        self.core.finish_with_progress(
            &mut now,
            &mut || {
                std::thread::sleep(std::time::Duration::from_millis(1));
            },
            &mut |core| Self::pump(core, upstream, ledger),
        );
        if self.ledger.values().any(|waited| !*waited) {
            self.core.poison("SCOPE_ACTIVE_REGISTRATION");
        }
        self.report()
    }
    /// Separate from finish so the existing durable ACK is written only after
    /// resources settle, and BEFORE the ownership terminal/physical EOF.
    pub fn publish_terminal(&mut self, operation_clean: bool) -> bool {
        if self.terminal_sent {
            return false;
        }
        self.terminal_sent = true;
        let clean = self.report().clean && operation_clean;
        if !clean {
            self.core.poison("SCOPE_OPERATION_FAILED");
        }
        if let Some(endpoint) = &mut self.upstream {
            if let Err(e) = endpoint.send_terminal(clean) {
                self.core.owner.borrow_mut().poison(e);
                return false;
            }
        }
        clean
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
    use super::*;
    use std::{cell::RefCell, os::unix::process::ExitStatusExt};
    #[test]
    fn native_repair_admission_clock_failure_zeroes_retained_sibling() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut core = Core::new(Rc::default(), 500_000_000);
        core.admit(1, 0, |_| Ok(fake("a", events.clone()))).unwrap();
        assert!(core.admission_clock(&mut || Err(error("INERT_CLOCK"))).is_err());
        assert_eq!(core.cancel_end, Some(0));
        assert_eq!(core.slots[0].completion_end, Some(0));
        assert_eq!(core.owner.borrow().first_fault, Some(ProtocolError("SCOPE_CLOCK")));
    }
    #[test]
    fn native_repair_unattached_entry_does_not_install_signal_handlers() {
        let source = include_str!("scope.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        let entry = source.split("pub fn entry_from_environment").nth(1).unwrap().split("pub fn hard_end").next().unwrap();
        assert!(entry.find("attachment::consume").unwrap() < entry.find("signals::install").unwrap());
    }
    #[test]
    fn native_repair_public_metadata_turn_does_not_scan_all_history() {
        let source = include_str!("scope.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!source.contains("for slot in &mut core.slots"));
    }
    #[test]
    fn native_repair_dispatch_enters_durable_guard_without_preparation_child() {
        let source = include_str!("../bin/keito-capture-guard.rs");
        assert!(!source.contains("fn dispatch_guard("));
    }
    struct Fake {
        events: Rc<RefCell<Vec<String>>>,
        name: &'static str,
        facts: EdgeFacts,
        begun: bool,
        fail_begin: bool,
        begin_ends: Vec<u64>,
    }
    impl Edge for Fake {
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
        }
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
