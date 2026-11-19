//! Private SQLite admission budget. Preserve the bundled 500ms/backoff/14-sleep
//! ceiling while accounting for actual monotonic elapsed time, including OS
//! oversleep. Scheduling can still delay a callback; this is not a hard OS bound.
use std::time::Duration;

const BUDGET: Duration = Duration::from_millis(500);
const DELAYS_MS: [u64; 14] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100, 100, 72];

#[derive(Default)]
pub(crate) struct BusyBudget {
    operation: Option<Operation>,
}

struct Operation {
    started: Duration,
    last_observed: Duration,
    next_count: i32,
}

impl BusyBudget {
    pub(crate) fn retry(
        &mut self,
        count: i32,
        mut now: impl FnMut() -> Option<Duration>,
        mut sleep: impl FnMut(Duration),
    ) -> bool {
        let Some(delay) = usize::try_from(count)
            .ok()
            .and_then(|index| DELAYS_MS.get(index))
        else {
            self.operation = None;
            return false;
        };
        let Some(before) = now() else {
            self.operation = None;
            return false;
        };
        // SQLite count zero starts a distinct locking operation. Never reset
        // the budget for later callbacks belonging to that operation.
        if count == 0 {
            self.operation = Some(Operation {
                started: before,
                last_observed: before,
                next_count: 0,
            });
        }
        let Some(operation) = &mut self.operation else {
            return false;
        };
        if count != operation.next_count || before < operation.last_observed {
            self.operation = None;
            return false;
        }
        let Some(remaining) = before
            .checked_sub(operation.started)
            .and_then(|elapsed| BUDGET.checked_sub(elapsed))
            .filter(|remaining| !remaining.is_zero())
        else {
            self.operation = None;
            return false;
        };
        sleep(Duration::from_millis(*delay).min(remaining));
        let Some(after) = now() else {
            self.operation = None;
            return false;
        };
        if after < before
            || after
                .checked_sub(operation.started)
                .is_none_or(|elapsed| elapsed >= BUDGET)
        {
            self.operation = None;
            return false;
        }
        operation.last_observed = after;
        operation.next_count += 1;
        true
    }
}

// Optional in-process wake hints only. Every wake returns to SQLite's ORIGINAL
// busy callback/count; BusyBudget owns all500ms/14callback refusal decisions.
#[derive(Clone)]
pub(crate) struct ReleaseNotifier(std::sync::Arc<WakeQueue>);
struct WakeQueue {
    waiters: std::sync::Mutex<std::collections::VecDeque<std::sync::Weak<Waiter>>>,
}
struct Waiter {
    state: std::sync::Mutex<WaitState>,
    ready: std::sync::Condvar,
    until: std::time::Instant,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum WaitState {
    Waiting,
    Selected,
    Retired,
}
#[cfg(unix)]
#[derive(PartialEq, Eq)]
struct DatabaseKey {
    path: std::path::PathBuf,
    device: u64,
    inode: u64,
}
#[cfg(unix)]
type Registry = Vec<(DatabaseKey, std::sync::Weak<WakeQueue>)>;
#[cfg(unix)]
const MAX_DATABASE_HINTS: usize = 256;
const MAX_WAIT_HINTS: usize = 4096;
#[cfg(unix)]
static WAKE_REGISTRY: std::sync::OnceLock<std::sync::Mutex<Registry>> = std::sync::OnceLock::new();
impl ReleaseNotifier {
    pub(crate) fn for_database(path: &std::path::Path) -> Option<Self> {
        #[cfg(unix)]
        {
            Self::for_registry(path, WAKE_REGISTRY.get_or_init(Default::default))
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }
    #[cfg(unix)]
    fn for_registry(path: &std::path::Path, registry: &std::sync::Mutex<Registry>) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let path = path.canonicalize().ok()?;
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let key = DatabaseKey {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        let mut registry = registry.try_lock().ok()?;
        registry.retain(|(_, weak)| weak.strong_count() != 0);
        if let Some(queue) = registry
            .iter()
            .find_map(|(candidate, weak)| (candidate == &key).then(|| weak.upgrade()).flatten())
        {
            return Some(Self(queue));
        }
        if registry.len() >= MAX_DATABASE_HINTS || registry.try_reserve(1).is_err() {
            return None;
        }
        let queue = std::sync::Arc::new(WakeQueue {
            waiters: std::sync::Mutex::new(std::collections::VecDeque::new()),
        });
        registry.push((key, std::sync::Arc::downgrade(&queue)));
        Some(Self(queue))
    }
    fn register(&self, until: std::time::Instant) -> Option<std::sync::Arc<Waiter>> {
        let waiter = std::sync::Arc::new(Waiter {
            state: std::sync::Mutex::new(WaitState::Waiting),
            ready: std::sync::Condvar::new(),
            until,
        });
        let mut queue = self.0.waiters.try_lock().ok()?;
        queue.retain(|weak| weak.strong_count() != 0);
        if queue.len() >= MAX_WAIT_HINTS || queue.try_reserve(1).is_err() {
            return None;
        }
        queue.push_back(std::sync::Arc::downgrade(&waiter));
        Some(waiter)
    }
    pub(crate) fn wait(&self, requested: Duration) {
        let Some(until) = std::time::Instant::now().checked_add(requested) else {
            std::thread::sleep(requested);
            return;
        };
        let fallback =
            || std::thread::sleep(until.saturating_duration_since(std::time::Instant::now()));
        let Some(waiter) = self.register(until) else {
            fallback();
            return;
        };
        let Ok(mut state) = waiter.state.try_lock() else {
            fallback();
            return;
        };
        loop {
            if *state == WaitState::Selected || *state == WaitState::Retired {
                *state = WaitState::Retired;
                return;
            }
            let remaining = until.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                *state = WaitState::Retired;
                return;
            }
            match waiter.ready.wait_timeout(state, remaining) {
                Ok((next, _)) => state = next,
                Err(error) => {
                    let (mut next, _) = error.into_inner();
                    *next = WaitState::Retired;
                    drop(next);
                    fallback();
                    return;
                }
            }
            // Spurious/cross-queue wakes cannot return this callback early:
            // keep the SAME absolute timer unless this node was selected.
        }
    }
    // Only successful writer commit + subsequent original AuthorityGuard drop
    // call this. Error/rollback/unknown-release paths emit NO scheduling hint.
    pub(crate) fn released_after_authority(&self) {
        let Ok(mut queue) = self.0.waiters.try_lock() else {
            return;
        };
        while let Some(weak) = queue.pop_front() {
            let Some(waiter) = weak.upgrade() else {
                continue;
            };
            let Ok(mut state) = waiter.state.try_lock() else {
                continue;
            };
            if *state != WaitState::Waiting {
                continue;
            }
            if std::time::Instant::now() >= waiter.until {
                *state = WaitState::Retired;
                continue;
            }
            *state = WaitState::Selected;
            drop(state);
            drop(queue);
            // Per-node condition variable: never wake an arbitrary ticket.
            waiter.ready.notify_one();
            return;
        }
        // No already-waiting node: discard, never bank a future retry credit.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn tick(policy: &mut BusyBudget, count: i32, clock: &Cell<u64>) -> bool {
        policy.retry(
            count,
            || Some(Duration::from_millis(clock.get())),
            |delay| clock.set(clock.get() + delay.as_millis() as u64),
        )
    }

    #[test]
    fn normal_backoff_retains_500ms_requested_sleep_and_fourteen_sleep_ceiling() {
        let mut policy = BusyBudget::default();
        let clock = Cell::new(0);
        let mut sleeps = Vec::new();
        for count in 0..=14 {
            if !policy.retry(
                count,
                || Some(Duration::from_millis(clock.get())),
                |delay| {
                    sleeps.push(delay.as_millis() as u64);
                    clock.set(clock.get() + delay.as_millis() as u64);
                },
            ) {
                break;
            }
        }
        // Independently state the bundled policy, rather than comparing the
        // implementation against its own schedule constant.
        assert_eq!(
            sleeps,
            [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100, 100, 72]
        );
        assert_eq!(Duration::from_millis(clock.get()), BUDGET);
        assert!(!tick(&mut policy, 14, &clock));
    }

    #[test]
    fn one_oversleep_stops_without_an_additional_lock_retry() {
        let mut policy = BusyBudget::default();
        let clock = Cell::new(100);
        let retry = policy.retry(
            0,
            || Some(Duration::from_millis(clock.get())),
            |_| clock.set(701),
        );
        assert!(
            !retry,
            "a 601ms first sleep exhausted the 500ms operation budget"
        );
    }

    #[test]
    fn scheduler_delay_between_callbacks_does_not_receive_a_fresh_budget() {
        let mut policy = BusyBudget::default();
        let clock = Cell::new(100);
        assert!(tick(&mut policy, 0, &clock));
        clock.set(650);
        let mut slept = false;
        let retry = policy.retry(
            1,
            || Some(Duration::from_millis(clock.get())),
            |_| slept = true,
        );
        assert!(!retry, "callback one must retain callback zero's deadline");
        assert!(
            !slept,
            "an expired operation must not request another sleep"
        );
    }

    #[test]
    fn remaining_elapsed_budget_caps_the_next_requested_sleep() {
        let mut policy = BusyBudget::default();
        let clock = Cell::new(0);
        assert!(tick(&mut policy, 0, &clock));
        clock.set(499);
        let mut requested = None;
        let retry = policy.retry(
            1,
            || Some(Duration::from_millis(clock.get())),
            |delay| {
                requested = Some(delay);
                clock.set(clock.get() + delay.as_millis() as u64);
            },
        );
        assert_eq!(requested, Some(Duration::from_millis(1)));
        assert!(!retry);
    }

    #[test]
    fn backwards_clock_between_callbacks_fails_closed_until_a_new_operation() {
        let mut policy = BusyBudget::default();
        let clock = Cell::new(100);
        assert!(tick(&mut policy, 0, &clock));
        clock.set(99);
        let mut slept = false;
        assert!(!policy.retry(
            1,
            || Some(Duration::from_millis(clock.get())),
            |_| slept = true,
        ));
        assert!(!slept);
        clock.set(102);
        assert!(
            !tick(&mut policy, 2, &clock),
            "same failed operation stays closed"
        );
        clock.set(10_000);
        assert!(
            tick(&mut policy, 0, &clock),
            "count zero starts a distinct operation"
        );
    }

    #[test]
    fn backwards_clock_during_sleep_does_not_restart_the_budget() {
        let mut policy = BusyBudget::default();
        let clock = Cell::new(100);
        assert!(!policy.retry(
            0,
            || Some(Duration::from_millis(clock.get())),
            |_| clock.set(99),
        ));
    }

    #[test]
    fn count_zero_resets_only_for_a_distinct_sqlite_locking_operation() {
        let mut policy = BusyBudget::default();
        let clock = Cell::new(100);
        for count in 0..=14 {
            if !tick(&mut policy, count, &clock) {
                break;
            }
        }
        clock.set(10_000);
        assert!(tick(&mut policy, 0, &clock));
        assert_eq!(clock.get(), 10_001);
        assert!(tick(&mut policy, 1, &clock));
        assert_eq!(clock.get(), 10_003);
    }

    fn refused_without_sleep(policy: &mut BusyBudget, count: i32) {
        let mut requested = Vec::new();
        assert!(!policy.retry(
            count,
            || Some(Duration::from_millis(10)),
            |delay| requested.push(delay)
        ));
        assert!(
            requested.is_empty(),
            "refused callback requested an unintended sleep"
        );
    }

    #[test]
    fn invalid_counts_clear_live_state_and_only_fresh_zero_can_recover() {
        for invalid in [-1, 14, 15, i32::MAX] {
            let clock = Cell::new(0);
            let mut policy = BusyBudget::default();
            assert!(tick(&mut policy, 0, &clock));
            refused_without_sleep(&mut policy, invalid);
            refused_without_sleep(&mut policy, 1);
            clock.set(20_000);
            assert!(tick(&mut policy, 0, &clock));
            assert_eq!(clock.get(), 20_001);
        }
    }

    #[test]
    fn initial_skipped_and_repeated_nonzero_counts_never_restart_a_budget() {
        let mut initial = BusyBudget::default();
        refused_without_sleep(&mut initial, 1);
        refused_without_sleep(&mut initial, 2);
        for repeat in [false, true] {
            let clock = Cell::new(0);
            let mut policy = BusyBudget::default();
            assert!(tick(&mut policy, 0, &clock));
            if repeat {
                assert!(tick(&mut policy, 1, &clock));
            }
            refused_without_sleep(&mut policy, if repeat { 1 } else { 2 });
            refused_without_sleep(&mut policy, 3);
            clock.set(20_000);
            assert!(tick(&mut policy, 0, &clock));
            assert_eq!(clock.get(), 20_001);
        }
    }

    #[test]
    fn unavailable_clock_before_sleep_clears_state_without_sleeping() {
        let clock = Cell::new(0);
        let mut policy = BusyBudget::default();
        assert!(tick(&mut policy, 0, &clock));
        let mut sleeps = 0;
        assert!(!policy.retry(1, || None, |_| sleeps += 1));
        assert_eq!(sleeps, 0);
        refused_without_sleep(&mut policy, 2);
        clock.set(20_000);
        assert!(tick(&mut policy, 0, &clock));
        assert_eq!(clock.get(), 20_001);
    }

    #[test]
    fn unavailable_clock_after_sleep_clears_state_without_an_extra_retry() {
        let mut policy = BusyBudget::default();
        let observations = Cell::new(0);
        let mut requested = Vec::new();
        assert!(!policy.retry(
            0,
            || {
                observations.set(observations.get() + 1);
                (observations.get() == 1).then_some(Duration::ZERO)
            },
            |delay| requested.push(delay)
        ));
        assert_eq!(observations.get(), 2);
        assert_eq!(requested, [Duration::from_millis(1)]);
        refused_without_sleep(&mut policy, 1);
        let clock = Cell::new(20_000);
        assert!(tick(&mut policy, 0, &clock));
        assert_eq!(clock.get(), 20_001);
    }
}

#[cfg(test)]
mod notify_one_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    fn queue() -> ReleaseNotifier {
        ReleaseNotifier(Arc::new(WakeQueue {
            waiters: Mutex::new(Default::default()),
        }))
    }
    fn future() -> std::time::Instant {
        std::time::Instant::now()
            .checked_add(Duration::from_secs(1))
            .unwrap()
    }
    #[test]
    fn release_selects_one_current_waiter_and_never_banks_credit() {
        let queue = queue();
        queue.released_after_authority();
        let a = queue.register(future()).unwrap();
        let b = queue.register(future()).unwrap();
        assert_eq!(*a.state.lock().unwrap() as u8, WaitState::Waiting as u8);
        queue.released_after_authority();
        assert_eq!(*a.state.lock().unwrap() as u8, WaitState::Selected as u8);
        assert_eq!(*b.state.lock().unwrap() as u8, WaitState::Waiting as u8);
        queue.released_after_authority();
        assert_eq!(*b.state.lock().unwrap() as u8, WaitState::Selected as u8);
        queue.released_after_authority();
        let c = queue.register(future()).unwrap();
        assert_eq!(*c.state.lock().unwrap() as u8, WaitState::Waiting as u8);
    }
    #[test]
    fn spurious_and_other_database_wakes_cannot_select_a_ticket() {
        let a = queue();
        let b = queue();
        let waiter = a.register(future()).unwrap();
        waiter.ready.notify_one();
        b.released_after_authority();
        assert_eq!(
            *waiter.state.lock().unwrap() as u8,
            WaitState::Waiting as u8
        );
        a.released_after_authority();
        assert_eq!(
            *waiter.state.lock().unwrap() as u8,
            WaitState::Selected as u8
        );
    }
    #[test]
    fn expired_and_retired_tickets_do_not_consume_the_next_release() {
        let queue = queue();
        let expired = queue.register(std::time::Instant::now()).unwrap();
        let retired = queue.register(future()).unwrap();
        *retired.state.lock().unwrap() = WaitState::Retired;
        let live = queue.register(future()).unwrap();
        queue.released_after_authority();
        assert_eq!(
            *expired.state.lock().unwrap() as u8,
            WaitState::Retired as u8
        );
        assert_eq!(*live.state.lock().unwrap() as u8, WaitState::Selected as u8);
    }
    #[test]
    fn queue_contention_and_capacity_fall_back_without_selecting_or_blocking() {
        let queue = queue();
        let live = queue.register(future()).unwrap();
        let mut locked = queue.0.waiters.lock().unwrap();
        assert!(queue.register(future()).is_none());
        queue.released_after_authority();
        assert_eq!(*live.state.lock().unwrap() as u8, WaitState::Waiting as u8);
        locked.clear();
        for _ in 0..MAX_WAIT_HINTS {
            locked.push_back(Arc::downgrade(&live));
        }
        drop(locked);
        assert!(queue.register(future()).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn database_registry_shares_only_the_same_live_file_identity() {
        let registry = Mutex::new(Vec::new());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let a_path = root.join("a.sqlite3");
        let b_path = root.join("b.sqlite3");
        std::fs::write(&a_path, []).unwrap();
        std::fs::write(&b_path, []).unwrap();
        let a = ReleaseNotifier::for_registry(&a_path, &registry).unwrap();
        let same = ReleaseNotifier::for_registry(&a_path, &registry).unwrap();
        let b = ReleaseNotifier::for_registry(&b_path, &registry).unwrap();
        let a_waiter = a.register(future()).unwrap();
        let b_waiter = b.register(future()).unwrap();
        b.released_after_authority();
        assert!(*a_waiter.state.lock().unwrap() == WaitState::Waiting);
        assert!(*b_waiter.state.lock().unwrap() == WaitState::Selected);
        same.released_after_authority();
        assert!(*a_waiter.state.lock().unwrap() == WaitState::Selected);
        let old_waiter = a.register(future()).unwrap();
        std::fs::rename(&a_path, root.join("retained-old.sqlite3")).unwrap();
        std::fs::write(&a_path, []).unwrap();
        let replaced = ReleaseNotifier::for_registry(&a_path, &registry).unwrap();
        replaced.released_after_authority();
        assert!(*old_waiter.state.lock().unwrap() == WaitState::Waiting);
        a.released_after_authority();
        assert!(*old_waiter.state.lock().unwrap() == WaitState::Selected);
    }
    #[test]
    fn poisoned_queue_disables_hints_without_panic_or_new_authority() {
        let queue = queue();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _locked = queue.0.waiters.lock().unwrap();
            panic!("controlled poison");
        }));
        assert!(queue.register(future()).is_none());
        queue.released_after_authority();
    }
}
