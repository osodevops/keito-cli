use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct Trace {
    calls: Vec<(&'static str, usize, u64)>,
}
struct Fake {
    id: usize,
    trace: Rc<RefCell<Trace>>,
    begin_error: bool,
    poll_error: bool,
    proof: bool,
    waited: bool,
}
impl Edge for Fake {
    fn begin(&mut self, end: u64) -> io::Result<()> {
        self.trace.borrow_mut().calls.push(("begin", self.id, end));
        if self.begin_error {
            Err(io::Error::other("synthetic begin"))
        } else {
            Ok(())
        }
    }
    fn poll(&mut self) -> io::Result<()> {
        self.trace.borrow_mut().calls.push(("poll", self.id, 0));
        if self.poll_error {
            Err(io::Error::other("synthetic poll"))
        } else {
            Ok(())
        }
    }
    fn proof(&self) -> bool {
        self.proof
    }
    fn waits_known(&self) -> bool {
        self.waited
    }
}
fn edge(trace: &Rc<RefCell<Trace>>, id: usize) -> Fake {
    Fake {
        id,
        trace: trace.clone(),
        begin_error: false,
        poll_error: false,
        proof: false,
        waited: false,
    }
}
fn registry() -> Registry<Fake> {
    Registry::new(SharedOwner::default(), 10_000_000_000)
}

#[test]
fn returned_edge_is_registered_before_the_actual_publisher_runs() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let id = r
        .admit(
            1,
            |_| Ok(edge(&trace, 0)),
            |id, slots| {
                assert_eq!(slots.len(), 1);
                assert_eq!(slots[id.index].id, 0);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(r.slot(&id).unwrap().id, 0);
}

#[test]
fn failed_publication_keeps_the_real_slot_and_closes_later_admission() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let error = r
        .admit(
            1,
            |_| Ok(edge(&trace, 0)),
            |_, _| Err(io::Error::other("ORIGINAL_RECEIPT")),
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "ORIGINAL_RECEIPT");
    assert_eq!(r.slots.len(), 1);
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("SCOPE_PUBLICATION"))
    );
    let mut created = false;
    assert!(r
        .admit(
            2,
            |_| {
                created = true;
                Ok(edge(&trace, 1))
            },
            |_, _| Ok(())
        )
        .is_err());
    assert!(!created);
    assert!(trace.borrow().calls.iter().any(|v| v.0 == "begin"));
}

#[test]
fn broadcasts_every_slot_before_any_poll_even_when_first_begin_fails() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let mut first = edge(&trace, 0);
    first.begin_error = true;
    r.slots.extend([first, edge(&trace, 1), edge(&trace, 2)]);
    r.begin(10, 500);
    assert_eq!(
        trace.borrow().calls,
        [("begin", 0, 500), ("begin", 1, 500), ("begin", 2, 500)]
    );
    r.poll();
    assert_eq!(
        trace
            .borrow()
            .calls
            .iter()
            .filter(|v| v.0 == "poll")
            .count(),
        3
    );
}

#[test]
fn one_poll_failure_cannot_skip_known_sibling_wait_collection_or_clear_slots() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let mut first = edge(&trace, 0);
    first.poll_error = true;
    let mut second = edge(&trace, 1);
    second.waited = true;
    r.slots.extend([first, second]);
    r.poll();
    assert_eq!(trace.borrow().calls, [("poll", 0, 0), ("poll", 1, 0)]);
    assert_eq!(r.slots.len(), 2);
}

#[test]
fn cleanup_bound_can_only_decrease_and_begin_does_not_wait() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    r.slots.push(edge(&trace, 0));
    r.begin(1, 500);
    r.begin(100, 800);
    r.begin(200, 300);
    assert_eq!(r.cleanup_end, Some(300));
    assert_eq!(trace.borrow().calls, [("begin", 0, 500), ("begin", 0, 300)]);
}

#[test]
fn waited_but_unproven_slot_is_retained_for_failed_final_report() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let mut first = edge(&trace, 0);
    first.waited = true;
    r.slots.push(first);
    r.poll();
    assert_eq!(r.slots.len(), 1);
    assert!(!r.slots[0].proof());
}

#[test]
fn cancelled_owner_never_calls_factory_and_foreign_slot_cannot_target_another_registry() {
    let mut a = registry();
    let mut b = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let id = a.admit(1, |_| Ok(edge(&trace, 0)), |_, _| Ok(())).unwrap();
    assert!(b.slot(&id).is_err());
    b.owner.borrow_mut().admission_closed = true;
    let mut called = false;
    assert!(b
        .admit(
            1,
            |_| {
                called = true;
                Ok(edge(&trace, 1))
            },
            |_, _| Ok(())
        )
        .is_err());
    assert!(!called);
}

#[test]
fn slot_limit_refuses_before_factory_and_preserves_first_owner_fault() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    for id in 0..MAX_REGISTRATIONS {
        r.slots.push(edge(&trace, id));
    }
    let mut called = false;
    assert!(r
        .admit(
            1,
            |_| {
                called = true;
                Ok(edge(&trace, 9999))
            },
            |_, _| Ok(())
        )
        .is_err());
    assert!(!called);
    assert_eq!(r.slots.len(), MAX_REGISTRATIONS);
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("SCOPE_SLOT_LIMIT"))
    );
}

#[test]
fn constructor_failure_cancels_already_retained_sibling_without_losing_original_error() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    r.slots.push(edge(&trace, 0));
    let error = r
        .admit(
            10,
            |_| Err(io::Error::other("ORIGINAL_SPAWN")),
            |_, _| Ok(()),
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "ORIGINAL_SPAWN");
    assert_eq!(r.slots.len(), 1);
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("SCOPE_SPAWN"))
    );
    assert_eq!(trace.borrow().calls[0].0, "begin");
}

#[test]
fn successful_finish_needs_proof_and_waits_under_the_same_fresh_bound() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let mut first = edge(&trace, 0);
    first.waited = true;
    r.slots.push(first);
    r.begin(10, 500);
    assert!(!r.finish_at(100));
    r.slots[0].proof = true;
    assert!(r.finish_at(101));
    let before = trace.borrow().calls.clone();
    r.begin(200, 900);
    assert_eq!(trace.borrow().calls, before);
    assert_eq!(r.cleanup_end, Some(500));
    assert!(r.owner.borrow().first_fault.is_none());
}

#[test]
fn expired_finish_retains_all_slots_and_never_upgrades_on_late_proof() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    r.slots.push(edge(&trace, 0));
    r.begin(10, 500);
    assert!(r.finish_at(500));
    r.slots[0].waited = true;
    r.slots[0].proof = true;
    r.poll();
    assert!(r.finish_at(501));
    assert_eq!(r.slots.len(), 1);
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("SCOPE_CLEANUP_DEADLINE"))
    );
}

#[test]
fn already_complete_group_facts_do_not_bypass_expired_scope_finish() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let mut first = edge(&trace, 0);
    first.waited = true;
    first.proof = true;
    r.slots.push(first);
    r.begin(10, 500);
    assert!(r.finish_at(501));
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("SCOPE_CLEANUP_DEADLINE"))
    );
}

#[test]
fn prior_owner_cause_survives_all_cleanup_faults_and_secondary_codes_are_bounded() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    r.owner
        .borrow_mut()
        .poison(ProtocolError("ORIGINAL_RECEIPT"));
    let mut first = edge(&trace, 0);
    first.begin_error = true;
    first.poll_error = true;
    r.slots.push(first);
    r.begin(10, 500);
    r.poll();
    for _ in 0..20 {
        r.poison("LATER_CLEANUP");
    }
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("ORIGINAL_RECEIPT"))
    );
    assert_eq!(
        r.secondary_faults,
        [
            ProtocolError("SCOPE_BEGIN"),
            ProtocolError("SCOPE_POLL"),
            ProtocolError("LATER_CLEANUP")
        ]
    );
}

#[test]
fn expired_admission_is_irreversible_and_broadcasts_to_retained_slots() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    r.slots.push(edge(&trace, 0));
    let mut created = false;
    assert!(r
        .admit(
            r.hard_end,
            |_| {
                created = true;
                Ok(edge(&trace, 1))
            },
            |_, _| Ok(())
        )
        .is_err());
    assert!(!created);
    assert!(r.owner.borrow().admission_closed);
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("SCOPE_ADMISSION"))
    );
    assert_eq!(trace.borrow().calls[0].0, "begin");
}

#[test]
fn sealed_finished_bound_is_not_silently_shortened_or_rebroadcast() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let mut first = edge(&trace, 0);
    first.waited = true;
    first.proof = true;
    r.slots.push(first);
    r.begin(10, 500);
    assert!(r.finish_at(100));
    let before = trace.borrow().calls.clone();
    r.begin(110, 200);
    assert_eq!(trace.borrow().calls, before);
    assert_eq!(r.cleanup_end, Some(500));
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("SCOPE_BOUND_SEALED"))
    );
}

#[test]
fn ordinary_publisher_unwind_keeps_the_inserted_slot_for_explicit_failure_cleanup() {
    let mut r = registry();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = r.admit(
            1,
            |_| Ok(edge(&trace, 0)),
            |_, _| panic!("synthetic publisher"),
        );
    }));
    assert!(result.is_err());
    assert_eq!(r.slots.len(), 1);
    r.poison("PUBLISHER_UNWIND");
    r.begin(2, 500);
    r.poll();
    assert_eq!(trace.borrow().calls, [("begin", 0, 500), ("poll", 0, 0)]);
    assert_eq!(r.slots.len(), 1);
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("PUBLISHER_UNWIND"))
    );
    // This finite seam tests retention across unwind, not actual public Drop
    // or a process-cleanup proof. The latter requires the real API OS actor.
}

#[test]
fn distinct_secondary_faults_stop_at_eight_without_replacing_the_original() {
    let mut r = registry();
    r.poison("ORIGINAL");
    let reasons = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K"];
    for reason in reasons {
        r.poison(reason);
    }
    assert_eq!(
        r.owner.borrow().first_fault,
        Some(ProtocolError("ORIGINAL"))
    );
    assert_eq!(
        r.secondary_faults,
        reasons[..8]
            .iter()
            .copied()
            .map(ProtocolError)
            .collect::<Vec<_>>()
    );
    r.poison("A");
    assert_eq!(r.secondary_faults.len(), 8);
}
