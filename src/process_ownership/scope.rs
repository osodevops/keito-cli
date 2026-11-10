//! Explicit entry-thread command owner. No implicit/TLS runner or positive
//! capture authority. Cooperative edges and sealed product images remain pending.
use super::{
    group::{CleanupPolicy, GroupLease},
    io::{CommandOutput, IoFacts, IoPlan, PipeIo},
    protocol::{ProtocolError, SharedOwner, MAX_REGISTRATIONS},
};
use std::{io, path::Path, process::{Command, ExitStatus, Stdio}, rc::Rc};

const RESERVE_NS: u64 = 50_000_000;
const MAX_IO_BYTES: usize = 4 * 1024 * 1024;
const SLOTS_PER_TURN: usize = 8;
fn error(reason: &'static str) -> io::Error { io::Error::other(reason) }
fn now() -> io::Result<u64> {
    let mut clock = std::mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, clock.as_mut_ptr()) } != 0 {
        return Err(error("SCOPE_CLOCK"));
    }
    let clock = unsafe { clock.assume_init() };
    u64::try_from(clock.tv_sec).ok()
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|s| s.checked_add(clock.tv_nsec as u64))
        .ok_or_else(|| error("SCOPE_CLOCK"))
}

#[derive(Clone)]
pub struct SlotId { index: usize, scope: Rc<()> }
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
}
enum Stage { Ready, Park, Running, Cleaning, Done }
struct GroupEdge {
    lease: GroupLease,
    command: Option<Command>,
    pipes: PipeIo,
    stage: Stage,
    proof: bool,
    first_fault: Option<&'static str>,
}
impl GroupEdge {
    fn progress(&mut self) -> io::Result<()> {
        // Outputs progress even while PARK is pending. Each pipe has a fixed
        // per-turn byte/syscall cap, and no WouldBlock is accepted as EOF.
        self.pipes.poll();
        if let Some(reason) = self.pipes.facts().first_fault {
            self.first_fault.get_or_insert(reason);
            if !matches!(self.stage, Stage::Cleaning | Stage::Done) { return Err(error(reason)); }
        }
        match self.stage {
            Stage::Ready => {
                if self.lease.poll_ready()? {
                    let command = self.command.take().ok_or_else(|| error("SCOPE_COMMAND"))?;
                    let started = self.lease.start_pending(command);
                    // Even an error AFTER spawn must transfer every available
                    // pipe to this retained slot before returning the fault.
                    let attached = if self.lease.report().workload_spawned {
                        self.pipes.attach(self.lease.stdin(), self.lease.stdout(), self.lease.stderr())
                    } else { Ok(()) };
                    self.stage = Stage::Park;
                    started?;
                    attached?;
                }
            }
            Stage::Park => {
                if self.lease.poll_parked()? { self.stage = Stage::Running; }
            }
            Stage::Running => { self.lease.poll_workload()?; }
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
        if matches!(self.stage, Stage::Done) { return Ok(()); }
        self.stage = Stage::Cleaning;
        self.lease.begin_local_cleanup(end)
    }
    fn poll(&mut self) -> io::Result<()> {
        let result = self.progress();
        if result.is_err() {
            self.first_fault.get_or_insert(self.lease.report().first_fault
                .or(self.pipes.facts().first_fault).unwrap_or("SCOPE_EDGE"));
        }
        if let Some(reason) = self.first_fault { Err(error(reason)) } else { result }
    }
    fn facts(&self) -> EdgeFacts {
        let facts = self.lease.report();
        EdgeFacts {
            status: facts.workload_status,
            waits_known: facts.keeper_status.is_some() && (!facts.workload_spawned || facts.workload_status.is_some()),
            group_absent: facts.group_absent,
            terminal_eof: facts.terminal_eof,
            resource_proof: self.proof,
            io_required: facts.workload_spawned,
            io: self.pipes.facts(),
            first_fault: self.first_fault.or(facts.first_fault).or(self.pipes.facts().first_fault),
        }
    }
    fn take_output(&mut self) -> CommandOutput { self.pipes.take_output() }
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
        Self { owner, identity: Rc::new(()), slots: vec![], hard_end, cancel_end: None,
            io_reserved: 0, cursor: 0, finish_called: false, finished: false }
    }
    fn poison(&mut self, reason: &'static str) { self.owner.borrow_mut().poison(ProtocolError(reason)); }
    fn admit(&mut self, at: u64, capacity: usize, create: impl FnOnce(SharedOwner) -> io::Result<E>) -> io::Result<SlotId> {
        let admission = self.owner.borrow().check_admission();
        if let Err(reason) = admission { return Err(error(reason.0)); }
        if self.finished || self.cancel_end.is_some() || at >= self.hard_end.saturating_sub(RESERVE_NS) {
            self.poison("SCOPE_ADMISSION"); return Err(error("SCOPE_ADMISSION"));
        }
        let reserved = self.io_reserved.checked_add(capacity).filter(|n| *n <= MAX_IO_BYTES);
        if self.slots.len() >= MAX_REGISTRATIONS || reserved.is_none() || self.slots.try_reserve(1).is_err() {
            self.poison("SCOPE_LIMIT"); return Err(error("SCOPE_LIMIT"));
        }
        let edge = match create(self.owner.clone()) {
            Ok(edge) => edge,
            Err(_) => { self.poison("SCOPE_SPAWN"); return Err(error("SCOPE_SPAWN")); }
        };
        let id = SlotId { index: self.slots.len(), scope: self.identity.clone() };
        // Capacity exists before factory spawn; the returned edge/handles are
        // retained before any subsequent publication or flags operation.
        self.slots.push(Slot { edge, completion_end: None, accepted_at: None, output_taken: false });
        self.io_reserved = reserved.unwrap();
        Ok(id)
    }
    fn index(&mut self, id: &SlotId) -> io::Result<usize> {
        if !Rc::ptr_eq(&id.scope, &self.identity) || id.index >= self.slots.len() {
            self.poison("SCOPE_SLOT_ID"); return Err(error("SCOPE_SLOT_ID"));
        }
        Ok(id.index)
    }
    fn request_complete(&mut self, id: &SlotId, at: u64) -> io::Result<()> {
        let index = self.index(id)?;
        if self.slots[index].completion_end.is_some() { return Ok(()); }
        // Migration baseline: inherited batch completion closes the owner.
        // The acceptance test below requires slot-local completion instead.
        self.owner.borrow_mut().admission_closed = true;
        let end = self.hard_end.min(at.saturating_add(RESERVE_NS));
        self.slots[index].completion_end = Some(end);
        let result = self.slots[index].edge.begin(end);
        if result.is_err() {
            self.poison(self.slots[index].edge.facts().first_fault.unwrap_or("SCOPE_BEGIN"));
        }
        result
    }
    fn cancel(&mut self, reason: Option<&'static str>, at: u64, requested: u64) {
        if let Some(reason) = reason { self.poison(reason); }
        self.owner.borrow_mut().admission_closed = true;
        let end = self.cancel_end.unwrap_or(u64::MAX).min(self.hard_end).min(requested).min(at.saturating_add(RESERVE_NS));
        if self.cancel_end == Some(end) { return; }
        self.cancel_end = Some(end);
        // Broadcast ALL intents before any poll/wait, even when begin fails.
        for index in 0..self.slots.len() {
            let slot = &mut self.slots[index];
            if slot.accepted_at.is_some() { continue; }
            slot.completion_end = Some(slot.completion_end.unwrap_or(u64::MAX).min(end));
            if slot.edge.begin(slot.completion_end.unwrap()).is_err() {
                let reason = slot.edge.facts().first_fault.unwrap_or("SCOPE_BEGIN");
                self.poison(reason);
            }
        }
    }
    fn poll(&mut self, clock: &mut impl FnMut() -> io::Result<u64>) {
        let at = match clock() { Ok(at) => at, Err(_) => { self.cancel(Some("SCOPE_CLOCK"), 0, 0); 0 } };
        let fault = self.owner.borrow().first_fault;
        if fault.is_some() && self.cancel_end.is_none() { self.cancel(None, at, self.hard_end); }
        if at >= self.hard_end.saturating_sub(RESERVE_NS) && self.cancel_end.is_none() {
            self.cancel(Some("SCOPE_OPERATION_DEADLINE"), at, self.hard_end);
        }
        let len = self.slots.len();
        for _ in 0..len.min(SLOTS_PER_TURN) {
            let index = self.cursor % len;
            self.cursor = (index + 1) % len;
            if self.slots[index].accepted_at.is_some() { continue; }
            if self.slots[index].edge.poll().is_err() {
                self.poison(self.slots[index].edge.facts().first_fault.unwrap_or("SCOPE_POLL"));
                let at = clock().unwrap_or(0);
                self.cancel(None, at, if at == 0 { 0 } else { self.hard_end });
            }
            let facts = self.slots[index].edge.facts();
            if facts.status.is_some() && self.slots[index].completion_end.is_none() {
                let at = clock().unwrap_or(0);
                if at == 0 { self.cancel(Some("SCOPE_CLOCK"), 0, 0); }
                else {
                    let id = SlotId { index, scope: self.identity.clone() };
                    if self.request_complete(&id, at).is_err() { self.cancel(None, at, self.hard_end); }
                }
            }
            if facts.resources_complete() {
                let end = self.slots[index].completion_end.unwrap_or(0).min(self.hard_end);
                match clock() {
                    Ok(at) if at < end => self.slots[index].accepted_at = Some(at),
                    _ => { self.poison("SCOPE_PROOF_DEADLINE"); self.cancel(None, 0, 0); }
                }
            }
        }
    }
    fn all_resources(&self) -> bool { self.slots.iter().all(|s| s.accepted_at.is_some()) }
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
pub struct CommandResult { pub status: ExitStatus, pub output: CommandOutput }
pub struct Scope { core: Core<GroupEdge> }
impl Scope {
    pub fn with_owner(owner: SharedOwner, hard_end: u64) -> Self { Self { core: Core::new(owner, hard_end) } }
    /// QA-owned700 image constructor only. A sealed official-pair image entry
    /// point is still required before wiring installed product call sites.
    pub fn start_owned_opaque(&mut self, mut command: Command, plan: IoPlan, root: &Path, image: &Path) -> io::Result<SlotId> {
        let at = now()?;
        let capacity = plan.capacity();
        let pipes = plan.prepare()?;
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let hard_end = self.core.hard_end;
        let result = self.core.admit(at, capacity, |owner| {
            let lease = GroupLease::spawn_keeper_with_policy(owner, root, image, hard_end, CleanupPolicy::HookShort)?;
            Ok(GroupEdge { lease, command: Some(command), pipes, stage: Stage::Ready, proof: false, first_fault: None })
        });
        if result.is_err() { self.core.cancel(None, now().unwrap_or(0), hard_end); }
        result
    }
    pub fn poll(&mut self) -> ScopeReport { self.core.poll(&mut now); self.report() }
    pub fn request_complete(&mut self, id: &SlotId) -> io::Result<()> { self.core.request_complete(id, now()?) }
    pub fn take_result(&mut self, id: &SlotId) -> io::Result<CommandResult> {
        let index = self.core.index(id)?;
        if let Some(reason) = self.core.owner.borrow().first_fault { return Err(error(reason.0)); }
        let slot = &mut self.core.slots[index];
        if slot.accepted_at.is_none() || slot.output_taken { return Err(error("SCOPE_RESULT")); }
        let status = slot.edge.facts().status.ok_or_else(|| error("SCOPE_RESULT"))?;
        slot.output_taken = true;
        Ok(CommandResult { status, output: slot.edge.take_output() })
    }
    pub fn begin_cancel(&mut self, reason: ProtocolError, requested_end: u64) {
        match now() {
            Ok(at) => self.core.cancel(Some(reason.0), at, requested_end),
            Err(_) => self.core.cancel(Some("SCOPE_CLOCK"), 0, 0),
        }
    }
    pub fn finish(&mut self) -> ScopeReport {
        self.core.finish_called = true;
        if self.core.finished { return self.report(); }
        let at = now().unwrap_or(0);
        if at == 0 { self.core.cancel(Some("SCOPE_CLOCK"), 0, 0); }
        else { self.core.cancel(None, at, self.core.hard_end); }
        loop {
            self.core.poll(&mut now);
            let at = now().unwrap_or(u64::MAX);
            if self.core.all_resources() { break; }
            if at >= self.core.cancel_end.unwrap_or(0) {
                self.core.poison("SCOPE_CLEANUP_DEADLINE"); break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.core.finished = true;
        self.report()
    }
    pub fn report(&self) -> ScopeReport {
        let first_fault = self.core.owner.borrow().first_fault;
        let all_resources = self.core.all_resources();
        ScopeReport { first_fault, admission_closed: self.core.owner.borrow().admission_closed,
            finish_called: self.core.finish_called, all_resources,
            clean: self.core.finished && self.core.finish_called && all_resources && first_fault.is_none(),
            slots: self.core.slots.iter().enumerate().map(|(slot,s)| {
                let facts = s.edge.facts();
                SlotReport { slot, status: facts.status, waits_known: facts.waits_known, group_absent: facts.group_absent,
                    terminal_eof: facts.terminal_eof, io_required: facts.io_required, io: facts.io,
                    accepted_at: s.accepted_at, completion_end: s.completion_end, first_fault: facts.first_fault }
            }).collect() }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if !self.core.finished {
            self.begin_cancel(ProtocolError("SCOPE_DROPPED"), self.core.hard_end);
            self.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, os::unix::process::ExitStatusExt};
    struct Fake {
        events: Rc<RefCell<Vec<String>>>, name: &'static str,
        facts: EdgeFacts, begun: bool, fail_begin: bool,
    }
    impl Edge for Fake {
        fn begin(&mut self, _end: u64) -> io::Result<()> {
            self.events.borrow_mut().push(format!("begin:{}",self.name)); self.begun = true;
            if self.fail_begin { Err(error("SCOPE_BEGIN")) } else { Ok(()) }
        }
        fn poll(&mut self) -> io::Result<()> {
            self.events.borrow_mut().push(format!("poll:{}",self.name));
            if self.begun {
                self.facts.status = Some(ExitStatus::from_raw(0)); self.facts.waits_known = true;
                self.facts.resource_proof = true; self.facts.group_absent = true; self.facts.terminal_eof = true;
            }
            Ok(())
        }
        fn facts(&self) -> EdgeFacts { self.facts }
        fn take_output(&mut self) -> CommandOutput { CommandOutput { stdout: vec![], stderr: vec![] } }
    }
    fn fake(name: &'static str, events: Rc<RefCell<Vec<String>>>) -> Fake {
        Fake { name, events, facts: EdgeFacts::default(), begun: false, fail_begin: false }
    }
    #[test]
    fn normal_completion_retains_a_then_admits_b_in_same_owner() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default(); let owner: SharedOwner = Rc::default();
        let mut core = Core::new(owner.clone(), 500_000_000);
        let a = core.admit(1, 1, |_| Ok(fake("a",events.clone()))).unwrap();
        core.request_complete(&a, 2).unwrap(); core.poll(&mut || Ok(3));
        assert!(core.slots[0].accepted_at.is_some());
        assert!(!owner.borrow().admission_closed, "local completion closed containing owner");
        let b = core.admit(4, 1, |_| Ok(fake("b",events.clone()))).unwrap();
        assert_eq!(a.index, 0); assert_eq!(b.index, 1); assert_eq!(core.slots.len(), 2);
        core.request_complete(&b, 5).unwrap(); core.poll(&mut || Ok(6));
        assert!(core.all_resources()); assert!(owner.borrow().first_fault.is_none());
    }
    #[test]
    fn cancellation_broadcasts_every_begin_before_poll_despite_first_begin_error() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut core = Core::new(Rc::default(), 500_000_000);
        core.admit(1, 1, |_| { let mut a = fake("a",events.clone()); a.fail_begin = true; Ok(a) }).unwrap();
        core.admit(1, 1, |_| Ok(fake("b",events.clone()))).unwrap();
        core.cancel(Some("ORIGINAL"), 2, 500_000_000); core.poll(&mut || Ok(3));
        assert_eq!(&events.borrow()[..4], ["begin:a","begin:b","poll:a","poll:b"]);
        assert_eq!(core.owner.borrow().first_fault, Some(ProtocolError("ORIGINAL")));
        assert!(core.all_resources()); assert!(core.admit(4,1,|_| Ok(fake("c",events.clone()))).is_err());
    }
    #[test]
    fn direct_exit_and_group_proof_do_not_replace_either_output_eof() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default(); let mut core = Core::new(Rc::default(),500_000_000);
        let id = core.admit(1,1,|_| { let mut a=fake("a",events); a.facts.io_required=true; Ok(a) }).unwrap();
        core.request_complete(&id,2).unwrap(); core.poll(&mut ||Ok(3));
        assert!(core.slots[0].edge.facts().waits_known); assert!(!core.all_resources());
        core.slots[0].edge.facts.io.stdin_closed=true; core.slots[0].edge.facts.io.stdout_eof=true;
        core.poll(&mut ||Ok(4)); assert!(!core.all_resources());
        core.slots[0].edge.facts.io.stderr_eof=true;
        core.poll(&mut ||Ok(5)); assert!(core.all_resources());
    }
    #[test]
    fn proof_equality_never_accepts_and_repeated_cancel_never_extends() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default(); let mut core = Core::new(Rc::default(),500_000_000);
        let id=core.admit(1,1,|_| Ok(fake("a",events))).unwrap();
        core.request_complete(&id,2).unwrap(); let end=core.slots[0].completion_end.unwrap();
        core.poll(&mut ||Ok(end)); assert!(!core.all_resources());
        assert!(core.owner.borrow().first_fault.is_some()); let bound=core.cancel_end;
        core.cancel(None,100_000_000,500_000_000); assert_eq!(core.cancel_end,bound);
    }
    #[test]
    fn slot_identity_and_whole_scope_buffer_budget_are_not_recycled() {
        let events: Rc<RefCell<Vec<String>>> = Rc::default(); let mut a=Core::new(Rc::default(),500_000_000);
        let mut b: Core<Fake>=Core::new(Rc::default(),500_000_000);
        let id=a.admit(1,MAX_IO_BYTES,|_| Ok(fake("a",events.clone()))).unwrap();
        assert!(b.index(&id).is_err());
        a.request_complete(&id,2).unwrap(); a.poll(&mut ||Ok(3));
        assert!(a.admit(4,1,|_| Ok(fake("b",events.clone()))).is_err());
        assert_eq!(a.slots.len(),1); assert_eq!(a.io_reserved,MAX_IO_BYTES);
    }
}
