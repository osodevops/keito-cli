//! Private fixed-output diagnostic; inert without its exact compile-time pin.
//! No runtime environment lookup, output capture or ownership authority.
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, sync::Mutex};

const EXPECTED: &str = "/Users/sionsmith/.t3/userdata/workspaces/keito-agent-capture-recovery-20261109-8bc862f6/evidence/installer-lifetime-diagnostic-first-20261112/installer-lifetime.json";
const CAPACITY: usize = 48;
fn enabled() -> bool {
    option_env!("KEITO_INSTALL_DIAG_RECEIPT") == Some(EXPECTED)
}
#[derive(Clone, Copy, serde::Serialize)]
struct Row {
    stage: u8,
    agent: u8,
    clock_ok: bool,
    at_ns: u64,
    hard_end_ns: u64,
    flags: u8,
    fault: u8,
}
const ZERO: Row = Row {
    stage: 0,
    agent: 0,
    clock_ok: false,
    at_ns: 0,
    hard_end_ns: 0,
    flags: 0,
    fault: 0,
};
struct State {
    armed: bool,
    used: usize,
    overflow: bool,
    rows: [Row; CAPACITY],
}
impl State {
    const fn new() -> Self {
        Self {
            armed: false,
            used: 0,
            overflow: false,
            rows: [ZERO; CAPACITY],
        }
    }
    fn append(&mut self, row: Row) {
        if self.used == CAPACITY {
            self.overflow = true;
            return;
        }
        self.rows[self.used] = row;
        self.used += 1;
    }
}
static STATE: Mutex<State> = Mutex::new(State::new());

/// Closed numeric stages; no argv, environment, error text or identities.
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum Stage {
    EntryAttached = 1,
    InstallBegin,
    PairReady,
    OwnerReady,
    MaterializeBegin,
    MaterializeEnd,
    DetectBegin,
    DetectEnd,
    LeafBegin,
    LeafEnd,
    PublishBegin,
    PublishEnd,
    ConfigPublished,
    DiscoveryPublished,
    OwnerFinishBegin,
    OwnerFinishEnd,
    MainFinishBegin,
    MainFinishEnd,
    TerminalDone,
}
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum Agent {
    None = 0,
    Codex = 1,
    Claude = 2,
}

/// Only source-derived static faults reach this whitelist. Unknown is numeric.
pub fn fault_code(reason: Option<&str>) -> u8 {
    match reason {
        None => 0,
        Some("SCOPE_ADMISSION") => 1,
        Some("SCOPE_OPERATION_DEADLINE") => 2,
        Some("SCOPE_PROOF_DEADLINE") => 3,
        Some("SCOPE_CLEANUP_DEADLINE") => 4,
        Some("SCOPE_CLOCK") => 5,
        Some("SCOPE_SPAWN") => 6,
        Some("GROUP_DEADLINE") => 7,
        Some("GROUP_LIFETIME") => 8,
        Some("SCOPE_ACTIVE_REGISTRATION") => 9,
        Some("SCOPE_PARENT_CANCELLED") => 10,
        Some("SCOPE_INTERRUPTED") => 11,
        Some(_) => 255,
    }
}
pub fn arm() {
    if enabled() {
        if let Ok(mut state) = STATE.lock() {
            state.armed = true;
        }
    }
}
/// flags bit0=Result::is_err; bit1=actual finish clean. No inferred proof.
pub fn record(stage: Stage, agent: Agent, hard_end_ns: u64, flags: u8, fault: u8) {
    if !enabled() {
        return;
    }
    let clock = crate::process_ownership::scope::monotonic_now();
    if let Ok(mut state) = STATE.lock() {
        state.append(Row {
            stage: stage as u8,
            agent: agent as u8,
            clock_ok: clock.is_ok(),
            at_ns: clock.unwrap_or(0),
            hard_end_ns,
            flags,
            fault,
        });
    }
}
/// Called once after original finish/terminal handling, before original stderr.
/// Failure is diagnostic NONPASS and never changes the operation's Result.
pub fn flush() -> bool {
    if !enabled() {
        return false;
    }
    let Ok(state) = STATE.lock() else {
        return false;
    };
    if !state.armed {
        return false;
    }
    let value = serde_json::json!({"schema_version":1,"diagnostic_only":true,
        "not_qualification":true,"overflow":state.overflow,"rows":&state.rows[..state.used]});
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return false;
    };
    if bytes.len() > 16384 {
        return false;
    }
    let Ok(mut file) = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(EXPECTED)
    else {
        return false;
    };
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installer_diagnostic_preserves_first_48_and_marks_overflow() {
        let mut state = State::new();
        for at_ns in 0..=CAPACITY as u64 {
            state.append(Row { at_ns, ..ZERO });
        }
        assert_eq!(state.used, CAPACITY);
        assert!(state.overflow);
        assert_eq!(state.rows[0].at_ns, 0);
        assert_eq!(state.rows[47].at_ns, 47);
    }
    #[test]
    fn installer_diagnostic_faults_do_not_serialize_error_text() {
        let canary = "PRIVATE_SECRET_/original/profile";
        let row = Row {
            fault: fault_code(Some(canary)),
            ..ZERO
        };
        let value = serde_json::to_string(&row).unwrap();
        assert_eq!(row.fault, 255);
        assert!(!value.contains(canary));
        assert_eq!(fault_code(Some("SCOPE_ADMISSION")), 1);
        assert_eq!(fault_code(Some("SCOPE_CLOCK")), 5);
        assert_eq!(fault_code(None), 0);
    }
}
