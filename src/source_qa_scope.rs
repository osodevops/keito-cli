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
    secondary_faults: Vec<ProtocolError>,
}
impl<E: Edge> Registry<E> {
    fn new(owner: SharedOwner, hard_end: u64) -> Self {
        Self {
            owner,
            identity: Rc::new(()),
            slots: Vec::new(),
            hard_end,
            cleanup_end: None,
            finished: false,
            secondary_faults: Vec::new(),
        }
    }
    fn poison(&mut self, reason: &'static str) {
        self.owner.borrow_mut().poison(ProtocolError(reason));
        if self.owner.borrow().first_fault != Some(ProtocolError(reason))
            && self.secondary_faults.len() < 8
            && !self.secondary_faults.contains(&ProtocolError(reason))
        {
            self.secondary_faults.push(ProtocolError(reason));
        }
    }
    fn admit(
        &mut self,
        at: u64,
        create: impl FnOnce(SharedOwner) -> io::Result<E>,
        publish: impl FnOnce(&SlotId, &[E]) -> io::Result<()>,
    ) -> io::Result<SlotId> {
        let admission = self.owner.borrow().check_admission();
        if let Err(error) = admission {
            self.begin(at, self.hard_end);
            return Err(io::Error::other(error.0));
        }
        if self.finished || self.cleanup_end.is_some() || at >= self.hard_end {
            self.poison("SCOPE_ADMISSION");
            self.begin(at, self.hard_end);
            return Err(io::Error::other("SCOPE_ADMISSION"));
        }
        if self.slots.len() >= MAX_REGISTRATIONS || self.slots.try_reserve(1).is_err() {
            self.poison("SCOPE_SLOT_LIMIT");
            self.begin(at, self.hard_end);
            return Err(io::Error::other("SCOPE_SLOT_LIMIT"));
        }
        let edge = match create(self.owner.clone()) {
            Ok(edge) => edge,
            Err(error) => {
                self.poison("SCOPE_SPAWN");
                self.begin(at, self.hard_end);
                return Err(error);
            }
        };
        let id = SlotId {
            index: self.slots.len(),
            registry: self.identity.clone(),
        };
        // Capacity was reserved BEFORE creating the handle. No fallible I/O or
        // publication occurs between factory return and local slot ownership.
        self.slots.push(edge);
        if let Err(error) = publish(&id, &self.slots) {
            self.poison("SCOPE_PUBLICATION");
            self.begin(at, self.hard_end);
            return Err(error);
        }
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
        if self.finished {
            if requested < self.cleanup_end.unwrap_or(self.hard_end) {
                self.poison("SCOPE_BOUND_SEALED");
            }
            return;
        }
        let end = requested
            .min(self.hard_end)
            .min(at.saturating_add(CLEANUP_NS))
            .min(self.cleanup_end.unwrap_or(u64::MAX));
        if self.cleanup_end == Some(end) {
            return;
        }
        self.cleanup_end = Some(end);
        let mut fault = false;
        for edge in &mut self.slots {
            if edge.begin(end).is_err() {
                fault = true;
            }
        }
        // Broadcast EVERY known slot before any poll/wait, including when one
        // begin fails. Individual edges may already have set a specific cause.
        if fault {
            self.poison("SCOPE_BEGIN");
        }
    }
    fn poll(&mut self) {
        let mut fault = false;
        for edge in &mut self.slots {
            if edge.poll().is_err() {
                fault = true;
            }
        }
        if fault {
            self.poison("SCOPE_POLL");
        }
        // Keep successful and failed slots alike. A wait is a fact, never a
        // substitute for group proof or permission to erase a failed handle.
    }
    fn finish_at(&mut self, at: u64) -> bool {
        if self.finished {
            return true;
        }
        if at >= self.cleanup_end.unwrap_or(0) {
            self.poison("SCOPE_CLEANUP_DEADLINE");
            self.finished = true;
        } else if self.slots.iter().all(Edge::waits_known)
            && (self.slots.iter().all(Edge::proof) || self.owner.borrow().first_fault.is_some())
        {
            self.finished = true;
        }
        self.finished
    }
}

/// Group-only observations. This is deliberately not a whole-Scope proof.
#[derive(Clone, Debug)]
pub struct RegisteredGroupReport {
    pub slot: usize,
    pub facts: GroupReport,
    pub group_proof: bool,
}

#[derive(Clone, Debug)]
pub struct RegistryReport {
    pub first_fault: Option<ProtocolError>,
    pub secondary_faults: Vec<ProtocolError>,
    pub admission_closed: bool,
    pub retained_slots: usize,
    pub groups: Vec<RegisteredGroupReport>,
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
        Self {
            inner: Registry::new(owner, hard_end),
        }
    }
    pub fn spawn_group(
        &mut self,
        root: &Path,
        keeper: &Path,
        publish: impl FnOnce(&SlotId, &GroupReport) -> io::Result<()>,
    ) -> io::Result<SlotId> {
        let at = match now() {
            Ok(at) => at,
            Err(error) => {
                self.inner.poison("SCOPE_CLOCK");
                self.inner.begin(0, 0);
                return Err(error);
            }
        };
        let hard_end = self.inner.hard_end;
        self.inner.admit(
            at,
            |owner| {
                GroupLease::spawn_keeper(owner, root, keeper, hard_end).map(|lease| GroupSlot {
                    lease,
                    proof: false,
                })
            },
            |id, edges| {
                // Publication sees only an actually retained lease's report.
                let edge = edges
                    .get(id.index)
                    .ok_or_else(|| io::Error::other("SCOPE_NOT_REGISTERED"))?;
                publish(id, &edge.lease.report())
            },
        )
    }
    pub fn await_ready(&mut self, id: &SlotId) -> io::Result<()> {
        self.with_slot(id, |slot| slot.lease.await_ready())
    }
    pub fn start(&mut self, id: &SlotId, command: Command) -> io::Result<()> {
        self.with_slot(id, |slot| slot.lease.start(command))
    }
    fn with_slot<T>(
        &mut self,
        id: &SlotId,
        action: impl FnOnce(&mut GroupSlot) -> io::Result<T>,
    ) -> io::Result<T> {
        let admission = self.inner.owner.borrow().check_admission();
        let result = admission
            .map_err(|e| io::Error::other(e.0))
            .and_then(|()| self.inner.slot(id).and_then(action));
        if result.is_err() {
            self.inner.poison("SCOPE_EDGE_OPERATION");
            self.begin_cleanup(self.inner.hard_end);
        }
        result
    }
    pub fn group_report(&self, id: &SlotId) -> io::Result<GroupReport> {
        if !Rc::ptr_eq(&id.registry, &self.inner.identity) || id.index >= self.inner.slots.len() {
            return Err(io::Error::other("SCOPE_SLOT_ID"));
        }
        Ok(self.inner.slots[id.index].lease.report())
    }
    pub fn begin_cleanup(&mut self, deadline: u64) {
        match now() {
            Ok(at) => self.inner.begin(at, deadline),
            Err(_) => {
                self.inner.poison("SCOPE_CLOCK");
                self.inner.begin(0, 0);
            }
        }
    }
    pub fn poll_cleanup(&mut self) -> RegistryReport {
        if self.inner.cleanup_end.is_none() {
            self.begin_cleanup(self.inner.hard_end);
        }
        self.inner.poll();
        self.report()
    }
    pub fn report(&self) -> RegistryReport {
        let owner = self.inner.owner.borrow();
        let groups: Vec<_> = self
            .inner
            .slots
            .iter()
            .enumerate()
            .map(|(slot, edge)| RegisteredGroupReport {
                slot,
                facts: edge.lease.report(),
                group_proof: edge.proof,
            })
            .collect();
        let all_group_proofs = groups.iter().all(|g| g.group_proof);
        let all_direct_waits_known = groups.iter().all(|g| {
            g.facts.keeper_status.is_some()
                && (!g.facts.workload_spawned || g.facts.workload_status.is_some())
        });
        RegistryReport {
            first_fault: owner.first_fault,
            admission_closed: owner.admission_closed,
            secondary_faults: self.inner.secondary_faults.clone(),
            retained_slots: self.inner.slots.len(),
            cleanup_end: self.inner.cleanup_end,
            groups,
            all_group_proofs,
            all_direct_waits_known,
            finish_called: self.inner.finished,
            output_eof_proven: false,
        }
    }
    pub fn finish(&mut self) -> RegistryReport {
        if self.inner.finished {
            return self.report();
        }
        self.begin_cleanup(self.inner.hard_end);
        loop {
            // Always collect one round even if the proof bound already expired
            // or the clock failed. Retain those late facts without upgrading it.
            self.poll_cleanup();
            match now() {
                Ok(at) => {
                    if self.inner.finish_at(at) {
                        break;
                    }
                }
                Err(_) => {
                    self.inner.poison("SCOPE_CLOCK");
                    self.inner.finished = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
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
