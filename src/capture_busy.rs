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
        assert_eq!(sleeps, DELAYS_MS);
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
}
