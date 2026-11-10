//! Developer Scope integration: locally retained opaque-group slots.
//!
//! This layer does not yet own cooperative core edges or output-pipe EOF and
//! cannot issue upstream SETTLED or any capture/profile approval.
use crate::{
    source_qa_group::{GroupLease, GroupReport},
    source_qa_protocol::{ProtocolError, SharedOwner, MAX_REGISTRATIONS},
};
use std::{io, path::Path, process::Command, rc::Rc};

const CLEANUP_NS: u64 = 1_800_000_000;

fn now() -> io::Result<u64> {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, value.as_mut_ptr()) } != 0 {
        return Err(io::Error::other("SCOPE_CLOCK"));
    }
    let value = unsafe { value.assume_init() };
    u64::try_from(value.tv_sec)
        .ok()
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|s| s.checked_add(value.tv_nsec as u64))
        .ok_or_else(|| io::Error::other("SCOPE_CLOCK"))
}

#[derive(Clone, Debug)]
pub struct SlotId {
    index: usize,
    registry: Rc<()>,
}
impl SlotId {
    pub fn index(&self) -> usize {
        self.index
    }
}

trait Edge {
    fn begin(&mut self, end: u64) -> io::Result<()>;
    fn poll(&mut self) -> io::Result<()>;
    fn proof(&self) -> bool;
    fn waits_known(&self) -> bool;
}

struct GroupSlot {
    lease: GroupLease,
    proof: bool,
}
impl Edge for GroupSlot {
    fn begin(&mut self, end: u64) -> io::Result<()> {
        self.lease.begin_cleanup(end)
    }
    fn poll(&mut self) -> io::Result<()> {
        if self.lease.poll_cleanup()?.is_some() {
            self.proof = true;
        }
        Ok(())
    }
    fn proof(&self) -> bool {
        self.proof
    }
    fn waits_known(&self) -> bool {
        let facts = self.lease.report();
        facts.keeper_status.is_some()
            && (!facts.workload_spawned || facts.workload_status.is_some())
    }
}

struct Registry<E: Edge> {
    owner: SharedOwner,
    identity: Rc<()>,
    slots: Vec<E>,
    hard_end: u64,
    cleanup_end: Option<u64>,
    finished: bool,
}
impl<E: Edge> Registry<E> {
    fn new(owner: SharedOwner, hard_end: u64) -> Self {
        Self { owner, identity: Rc::new(()), slots: Vec::new(), hard_end,
               cleanup_end: None, finished: false }
    }
    fn poison(&mut self, reason: &'static str) {
        self.owner.borrow_mut().poison(ProtocolError(reason));
    }
    fn admit(
        &mut self,
        at: u64,
        create: impl FnOnce(SharedOwner) -> io::Result<E>,
        publish: impl FnOnce(&SlotId, &[E]) -> io::Result<()>,
    ) -> io::Result<SlotId> {
        self.owner.borrow().check_admission().map_err(|e| io::Error::other(e.0))?;
        if self.finished || self.cleanup_end.is_some() || at >= self.hard_end {
            return Err(io::Error::other("SCOPE_ADMISSION"));
        }
        if self.slots.len() >= MAX_REGISTRATIONS || self.slots.try_reserve(1).is_err() {
            self.poison("SCOPE_SLOT_LIMIT");
            return Err(io::Error::other("SCOPE_SLOT_LIMIT"));
        }
        let edge = create(self.owner.clone())?;
        let id = SlotId { index: self.slots.len(), registry: self.identity.clone() };
        publish(&id, &self.slots)?;
        self.slots.push(edge);
        Ok(id)
    }
    fn slot(&mut self, id: &SlotId) -> io::Result<&mut E> {
        if !Rc::ptr_eq(&id.registry, &self.identity) || id.index >= self.slots.len() {
            self.poison("SCOPE_SLOT_ID");
            return Err(io::Error::other("SCOPE_SLOT_ID"));
        }
        Ok(&mut self.slots[id.index])
    }
    fn begin(&mut self, at: u64, requested: u64) {
        self.owner.borrow_mut().admission_closed = true;
        let end = requested.min(self.hard_end).min(at.saturating_add(CLEANUP_NS));
        self.cleanup_end = Some(end);
        // Baseline migration of serial owner behavior; acceptance tests below
        // require the integration's broadcast-before-poll and failure retention.
        for edge in &mut self.slots {
            if edge.begin(end).is_err() {
                self.owner.borrow_mut().poison(ProtocolError("SCOPE_BEGIN"));
                return;
            }
            let _ = edge.poll();
        }
    }
    fn poll(&mut self) {
        for edge in &mut self.slots {
            if edge.poll().is_err() {
                self.owner.borrow_mut().poison(ProtocolError("SCOPE_POLL"));
                break;
            }
        }
        self.slots.retain(|edge| !edge.waits_known());
    }
}

/// Group-only observations. This is deliberately not a whole-Scope proof.
#[derive(Clone, Debug)]
pub struct RegistryReport {
    pub first_fault: Option<ProtocolError>,
    pub admission_closed: bool,
    pub retained_slots: usize,
    pub cleanup_end: Option<u64>,
    pub all_group_proofs: bool,
    pub all_direct_waits_known: bool,
    pub finish_called: bool,
    pub output_eof_proven: bool,
}

pub struct GroupRegistry {
    inner: Registry<GroupSlot>,
}
impl GroupRegistry {
    pub fn new(owner: SharedOwner, hard_end: u64) -> Self {
        Self { inner: Registry::new(owner, hard_end) }
    }
    pub fn spawn_group(
        &mut self,
        root: &Path,
        keeper: &Path,
        publish: impl FnOnce(&SlotId, &GroupReport) -> io::Result<()>,
    ) -> io::Result<SlotId> {
        let at = now()?;
        let hard_end = self.inner.hard_end;
        self.inner.admit(at, |owner| {
            GroupLease::spawn_keeper(owner, root, keeper, hard_end)
                .map(|lease| GroupSlot { lease, proof: false })
        }, |id, edges| {
            // Publication sees only an actually retained lease's report.
            let edge = edges.get(id.index).ok_or_else(|| io::Error::other("SCOPE_NOT_REGISTERED"))?;
            publish(id, &edge.lease.report())
        })
    }
    pub fn await_ready(&mut self, id: &SlotId) -> io::Result<()> {
        self.inner.slot(id)?.lease.await_ready()
    }
    pub fn start(&mut self, id: &SlotId, command: Command) -> io::Result<()> {
        self.inner.slot(id)?.lease.start(command)
    }
    pub fn begin_cleanup(&mut self, deadline: u64) {
        match now() {
            Ok(at) => self.inner.begin(at, deadline),
            Err(_) => { self.inner.poison("SCOPE_CLOCK"); self.inner.begin(0, 0); }
        }
    }
    pub fn poll_cleanup(&mut self) -> RegistryReport {
        self.inner.poll();
        self.report()
    }
    pub fn report(&self) -> RegistryReport {
        let owner = self.inner.owner.borrow();
        RegistryReport { first_fault: owner.first_fault, admission_closed: owner.admission_closed,
            retained_slots: self.inner.slots.len(), cleanup_end: self.inner.cleanup_end,
            all_group_proofs: self.inner.slots.iter().all(Edge::proof),
            all_direct_waits_known: self.inner.slots.iter().all(Edge::waits_known),
            finish_called: self.inner.finished, output_eof_proven: false }
    }
    pub fn finish(&mut self) -> RegistryReport {
        self.begin_cleanup(self.inner.hard_end);
        while now().is_ok_and(|at| at < self.inner.cleanup_end.unwrap_or(0)) {
            let report = self.poll_cleanup();
            if report.all_direct_waits_known { break; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.inner.finished = true;
        self.report()
    }
}
impl Drop for GroupRegistry {
    fn drop(&mut self) {
        if !self.inner.finished {
            self.inner.poison("SCOPE_DROPPED_WITHOUT_FINISH");
            let _ = self.finish();
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/source_qa_scope.rs"]
mod tests;
