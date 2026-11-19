//! Private denied/counting observers. Never compiled into an installed binary;
//! they cannot create a positive runtime, consent, profile or source capability.
use std::cell::RefCell;
thread_local! { static STATE: RefCell<Option<[usize;3]>> = const { RefCell::new(None) }; }
pub(crate) struct Denied;
impl Denied {
    pub(crate) fn arm() -> Self {
        STATE.with(|state| {
            assert!(state.borrow().is_none());
            *state.borrow_mut() = Some([0; 3]);
        });
        Self
    }
    pub(crate) fn counts(&self) -> [usize; 3] {
        STATE.with(|state| state.borrow().unwrap())
    }
}
impl Drop for Denied {
    fn drop(&mut self) {
        STATE.with(|state| *state.borrow_mut() = None);
    }
}
pub(crate) fn observe(index: usize) {
    STATE.with(|state| {
        if let Some(counts) = state.borrow_mut().as_mut() {
            counts[index] += 1;
            panic!("denied numeric profile/origin/source operation reached");
        }
    });
}

// A one-shot negative fault after the first SQLite queue/session writes. Only
// cfg(test) code can arm it; it never supplies a runtime/profile approval.
type WriteFault = Box<dyn FnOnce(&rusqlite::Connection)>;
thread_local! { static AFTER_WRITE: RefCell<Option<WriteFault>> = const {RefCell::new(None)}; }
pub(crate) struct AfterWrite;
impl AfterWrite {
    pub(crate) fn arm(action: impl FnOnce(&rusqlite::Connection) + 'static) -> Self {
        AFTER_WRITE.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(Box::new(action));
        });
        Self
    }
}
impl Drop for AfterWrite {
    fn drop(&mut self) {
        AFTER_WRITE.with(|slot| *slot.borrow_mut() = None);
    }
}
pub(crate) fn after_numeric_write(connection: &rusqlite::Connection) {
    let action = AFTER_WRITE.with(|slot| slot.borrow_mut().take());
    if let Some(action) = action {
        action(connection);
    }
}
