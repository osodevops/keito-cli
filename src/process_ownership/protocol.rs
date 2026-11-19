//! Shared Unix cancellation metadata. No capture, profile or auth authority.
//! Included in default library/guard builds and their real process owners.
//! Protocol proof alone is not a wait or process-group absence proof.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fmt,
    io::{Read, Write},
    net::Shutdown,
    os::{fd::AsRawFd, unix::net::UnixStream},
    rc::Rc,
};

pub const MAX_FRAME: usize = 256;
pub const MAX_PENDING: usize = 1024;
pub const MAX_READS: usize = 8;
pub const MAX_PARSE: usize = 32;
pub const MAX_REGISTRATIONS: usize = 4096;
pub const MAX_FRAMES: usize = 16384;
pub const MAX_WIRE: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolError(pub &'static str);
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ProtocolError {}
type Result<T> = std::result::Result<T, ProtocolError>;
pub(super) type Registration = (u32, u32, String);

#[derive(Default, Debug)]
pub struct OwnerState {
    pub first_fault: Option<ProtocolError>,
    pub admission_closed: bool,
    wire: usize,
    frames: usize,
    registrations: usize,
    budget_fault: Option<ProtocolError>,
}
pub type SharedOwner = Rc<RefCell<OwnerState>>;
impl OwnerState {
    pub fn check_admission(&self) -> Result<()> {
        if let Some(error) = self.first_fault {
            return Err(error);
        }
        if self.admission_closed {
            return Err(ProtocolError("OWNER_ADMISSION_CLOSED"));
        }
        Ok(())
    }
    pub fn poison(&mut self, error: ProtocolError) {
        self.first_fault.get_or_insert(error);
        self.admission_closed = true;
    }
    pub(super) fn charge(&mut self, kind: &'static str, amount: usize) -> Result<()> {
        if let Some(error) = self.budget_fault {
            return Err(error);
        }
        let (counter, cap, reason) = match kind {
            "wire" => (&mut self.wire, MAX_WIRE, "LIMIT_WIRE"),
            "frames" => (&mut self.frames, MAX_FRAMES, "LIMIT_FRAMES"),
            "registrations" => (
                &mut self.registrations,
                MAX_REGISTRATIONS,
                "LIMIT_REGISTRATIONS",
            ),
            _ => unreachable!("closed internal budget kind"),
        };
        match counter.checked_add(amount).filter(|n| *n <= cap) {
            Some(next) => *counter = next,
            None => {
                let error = ProtocolError(reason);
                self.budget_fault = Some(error);
                self.poison(error);
                return Err(error);
            }
        }
        Ok(())
    }
}

fn nonce_valid(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn unsigned(value: &str) -> Result<u64> {
    if value.is_empty()
        || value.len() > 20
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ProtocolError("INTEGER_SCHEMA"));
    }
    value.parse().map_err(|_| ProtocolError("INTEGER_RANGE"))
}
fn pid(value: &str) -> Result<u32> {
    let value = unsigned(value)?;
    if value == 0 || value > i32::MAX as u64 {
        return Err(ProtocolError("PID_SCHEMA"));
    }
    Ok(value as u32)
}
fn wait_status(value: &str) -> Result<()> {
    if value == "-0" {
        return Err(ProtocolError("INTEGER_SCHEMA"));
    }
    unsigned(value.strip_prefix('-').unwrap_or(value))?;
    value
        .parse::<i32>()
        .map_err(|_| ProtocolError("INTEGER_RANGE"))?;
    Ok(())
}

#[derive(Default, Debug)]
pub struct PollWork {
    pub reads: usize,
    pub parsed: usize,
}

pub struct Endpoint {
    socket: UnixStream,
    nonce: String,
    expected_pid: Option<u32>,
    pub owner: SharedOwner,
    pending: Vec<u8>,
    live: bool,
    terminal: bool,
    eof: bool,
    settled: bool,
    cancelled: bool,
    first_fault: Option<ProtocolError>,
    cancel_deadline_ns: Option<u64>,
    parent_role: Option<bool>,
    events: Vec<String>,
    registrations: BTreeMap<Registration, bool>,
    sent_registrations: BTreeMap<Registration, bool>,
    sent_live: bool,
    sent_terminal: bool,
    sent_cancel: bool,
    sent_cancel_end: Option<u64>,
    failed_attempted: bool,
    pub last_poll: PollWork,
}
impl Endpoint {
    // Readiness metadata only. The original receive() remains the sole parser
    // and physical EOF observer; a known EOF cannot wake another Child wait.
    pub(super) fn progress_wait_source(&self) -> Option<i32> {
        (!self.eof).then(|| self.socket.as_raw_fd())
    }
    pub(super) fn progress_buffered_event(&self) -> bool {
        !self.eof
            && (self.pending.contains(&b'\n')
                || self.last_poll.reads >= MAX_READS
                || self.last_poll.parsed >= MAX_PARSE)
    }
    /// Nonterminal metadata predicate only; never sends proof or closes a peer.
    pub(super) fn sent_resources_complete(&self) -> bool {
        self.sent_live
            && !self.sent_terminal
            && !self.cancelled
            && self.first_fault.is_none()
            && self.owner.borrow().first_fault.is_none()
            && self.pending.is_empty()
            && self.events.is_empty()
            && self.last_poll.reads < MAX_READS
            && self.last_poll.parsed < MAX_PARSE
            && self.sent_registrations.values().all(|waited| *waited)
    }
    pub(super) fn set_expected_child(&mut self, pid: u32) -> Result<()> {
        if self.live || self.expected_pid.is_some() || pid == 0 || pid > i32::MAX as u32 {
            return Err(self.poison(ProtocolError("PEER_IDENTITY")));
        }
        self.expected_pid = Some(pid);
        Ok(())
    }
    pub fn new(socket: UnixStream, nonce: String, expected_pid: Option<u32>) -> Result<Self> {
        Self::with_owner(socket, nonce, expected_pid, Rc::default())
    }
    pub fn with_owner(
        socket: UnixStream,
        nonce: String,
        expected_pid: Option<u32>,
        owner: SharedOwner,
    ) -> Result<Self> {
        if !nonce_valid(&nonce) || expected_pid.is_some_and(|p| p == 0 || p > i32::MAX as u32) {
            let error = ProtocolError("ENDPOINT_SCHEMA");
            owner.borrow_mut().poison(error);
            return Err(error);
        }
        if socket.set_nonblocking(true).is_err()
            || unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0
        {
            let error = ProtocolError("SOCKET_FLAGS");
            owner.borrow_mut().poison(error);
            return Err(error);
        }
        Ok(Self {
            socket,
            nonce,
            expected_pid,
            owner,
            pending: Vec::new(),
            live: false,
            terminal: false,
            eof: false,
            settled: false,
            cancelled: false,
            first_fault: None,
            cancel_deadline_ns: None,
            parent_role: None,
            events: Vec::new(),
            registrations: BTreeMap::new(),
            sent_registrations: BTreeMap::new(),
            sent_live: false,
            sent_terminal: false,
            sent_cancel: false,
            sent_cancel_end: None,
            failed_attempted: false,
            last_poll: PollWork::default(),
        })
    }
    fn poison(&mut self, error: ProtocolError) -> ProtocolError {
        let first = *self.first_fault.get_or_insert(error);
        self.settled = false;
        self.owner.borrow_mut().poison(first);
        first
    }
    fn check(&mut self) -> Result<()> {
        if let Some(error) = self.first_fault {
            return Err(error);
        }
        let budget_fault = self.owner.borrow().budget_fault;
        if let Some(error) = budget_fault {
            return Err(self.poison(error));
        }
        Ok(())
    }
    fn write_frame(&mut self, value: &str) -> Result<()> {
        let result = (|| {
            if !value.is_ascii() || value.len() + 34 > MAX_FRAME || value.contains(['\n', '\r']) {
                return Err(ProtocolError("FRAME_SCHEMA"));
            }
            let frame = format!("{}|{}\n", self.nonce, value);
            {
                let mut owner = self.owner.borrow_mut();
                owner.charge("wire", frame.len())?;
                owner.charge("frames", 1)?;
            }
            if self
                .socket
                .write(frame.as_bytes())
                .map_err(|_| ProtocolError("WRITE_FAILED"))?
                != frame.len()
            {
                return Err(ProtocolError("SHORT_WRITE"));
            }
            Ok(())
        })();
        result.map_err(|e| self.poison(e))
    }
    pub fn send_live(&mut self) -> Result<()> {
        self.check()?;
        if self.sent_live || self.sent_terminal || self.sent_cancel {
            return Err(self.poison(ProtocolError("SEND_STATE")));
        }
        self.sent_live = true;
        self.write_frame(&format!("LIVE|{}", std::process::id()))
    }
    pub fn send_cancel(&mut self, deadline_ns: u64) -> Result<()> {
        self.check()?;
        if deadline_ns == 0 || self.sent_live || self.sent_terminal || self.sent_cancel {
            return Err(self.poison(ProtocolError("CANCEL_STATE")));
        }
        self.sent_cancel = true;
        self.sent_cancel_end = Some(deadline_ns);
        self.write_frame(&format!("CANCEL|{}|{}", std::process::id(), deadline_ns))
    }
    pub fn send_cancel_tighter(&mut self, deadline_ns: u64) -> Result<()> {
        self.check()?;
        if deadline_ns == 0 || self.sent_terminal || self.sent_live {
            return Err(self.poison(ProtocolError("CANCEL_STATE")));
        }
        match self.sent_cancel_end {
            None => self.send_cancel(deadline_ns),
            Some(previous) if deadline_ns >= previous => Ok(()),
            Some(_) => {
                self.sent_cancel_end = Some(deadline_ns);
                self.write_frame(&format!("CANCEL|{}|{}", std::process::id(), deadline_ns))
            }
        }
    }
    pub fn send_terminal(&mut self, clean: bool) -> Result<()> {
        if clean {
            self.check()?;
            if self.owner.borrow().first_fault.is_some()
                || !self.sent_live
                || self.sent_registrations.values().any(|w| !w)
            {
                return Err(self.poison(ProtocolError("OWNER_FAULT_OR_ACTIVE_REGISTRATION")));
            }
        } else if self.failed_attempted {
            return Err(self.poison(ProtocolError("DUPLICATE_TERMINAL")));
        }
        if self.sent_terminal {
            return Err(self.poison(ProtocolError("DUPLICATE_TERMINAL")));
        }
        self.failed_attempted |= !clean;
        self.sent_terminal = true;
        self.write_frame(&format!(
            "{}|{}",
            if clean { "SETTLED" } else { "FAILED" },
            std::process::id()
        ))?;
        self.socket
            .shutdown(Shutdown::Write)
            .map_err(|_| self.poison(ProtocolError("SHUTDOWN_FAILED")))
    }
    pub fn metadata(&mut self, value: &str) -> Result<()> {
        self.check()?;
        let result = (|| {
            if !self.sent_live
                || self.sent_terminal
                || value.len() + 34 > MAX_FRAME
                || !value.is_ascii()
            {
                return Err(ProtocolError("METADATA_SEND_STATE"));
            }
            if Self::record_metadata(&mut self.sent_registrations, std::process::id(), value)? {
                self.owner.borrow_mut().charge("registrations", 1)?;
            }
            self.write_frame(value)
        })();
        result.map_err(|e| self.poison(e))
    }
    // The no-upstream root uses this SAME ledger validation as wire senders.
    // A missing observer does not mean registrations may be forgotten.
    pub(super) fn record_metadata(
        registrations: &mut BTreeMap<Registration, bool>,
        root: u32,
        value: &str,
    ) -> Result<bool> {
        let fields: Vec<_> = value.split('|').collect();
        if fields.len() != 5
            || !matches!(fields[0], "REGISTER" | "WAITED")
            || !nonce_valid(fields[3])
            || value.len() + 34 > MAX_FRAME
        {
            return Err(ProtocolError("METADATA_SCHEMA"));
        }
        let key = (pid(fields[1])?, pid(fields[2])?, fields[3].to_owned());
        if fields[0] == "REGISTER" {
            unsigned(fields[4])?;
            Self::validate_registration(registrations, root, &key)?;
            if registrations.len() >= MAX_REGISTRATIONS {
                return Err(ProtocolError("LIMIT_REGISTRATIONS"));
            }
            registrations.insert(key, false);
            Ok(true)
        } else {
            wait_status(fields[4])?;
            let waited = registrations
                .get_mut(&key)
                .filter(|w| !**w)
                .ok_or(ProtocolError("UNMATCHED_WAIT"))?;
            *waited = true;
            Ok(false)
        }
    }
    fn validate_registration(
        registrations: &BTreeMap<Registration, bool>,
        root: u32,
        key: &Registration,
    ) -> Result<()> {
        if key.0 != root
            && !registrations
                .iter()
                .any(|(k, waited)| k.1 == key.0 && !waited)
        {
            return Err(ProtocolError("OWNER_EDGE"));
        }
        if key.0 == key.1
            || key.1 == root
            || registrations.contains_key(key)
            || registrations
                .iter()
                .any(|(k, waited)| k.1 == key.1 && !waited)
        {
            return Err(ProtocolError("DUPLICATE_OR_CYCLIC_REGISTRATION"));
        }
        Ok(())
    }
    fn frame(&mut self, line: &[u8], parent: bool) -> Result<()> {
        if self.terminal || line.len() + 1 > MAX_FRAME {
            return Err(ProtocolError("POST_TERMINAL_OR_FRAME_LIMIT"));
        }
        self.owner.borrow_mut().charge("frames", 1)?;
        if !line.is_ascii() {
            return Err(ProtocolError("FRAME_ENCODING"));
        }
        let text = std::str::from_utf8(line).map_err(|_| ProtocolError("FRAME_ENCODING"))?;
        let fields: Vec<_> = text.split('|').collect();
        if !matches!(fields.len(), 3 | 4 | 6) || fields[0] != self.nonce {
            return Err(ProtocolError("NONCE_OR_SCHEMA"));
        }
        if fields.len() == 6 {
            if parent
                || !self.live
                || !matches!(fields[1], "REGISTER" | "WAITED")
                || !nonce_valid(fields[4])
            {
                return Err(ProtocolError("METADATA_STATE"));
            }
            let key = (pid(fields[2])?, pid(fields[3])?, fields[4].to_owned());
            if fields[1] == "REGISTER" {
                unsigned(fields[5])?;
                Self::validate_registration(
                    &self.registrations,
                    self.expected_pid.ok_or(ProtocolError("MISSING_PEER"))?,
                    &key,
                )?;
                self.owner.borrow_mut().charge("registrations", 1)?;
                self.registrations.insert(key, false);
            } else {
                wait_status(fields[5])?;
                let waited = self
                    .registrations
                    .get_mut(&key)
                    .filter(|w| !**w)
                    .ok_or(ProtocolError("UNMATCHED_WAIT"))?;
                *waited = true;
            }
            self.events.push(fields[1..].join("|"));
            return Ok(());
        }
        let peer = pid(fields[2])?;
        if self.expected_pid.is_some_and(|p| p != peer) {
            return Err(ProtocolError("PEER_IDENTITY"));
        }
        if parent {
            if fields.len() != 4 || fields[1] != "CANCEL" {
                return Err(ProtocolError("PARENT_STATE"));
            }
            let deadline = unsigned(fields[3])?;
            if deadline == 0 {
                return Err(ProtocolError("CANCEL_DEADLINE"));
            }
            self.cancel_deadline_ns =
                Some(self.cancel_deadline_ns.unwrap_or(deadline).min(deadline));
            self.cancelled = true;
            self.owner.borrow_mut().admission_closed = true;
            self.expected_pid = Some(peer);
        } else if fields.len() != 3 {
            return Err(ProtocolError("CHILD_SCHEMA"));
        } else {
            match fields[1] {
                "LIVE" if !self.live => {
                    self.live = true;
                    self.expected_pid = Some(peer);
                }
                "SETTLED" if self.live && self.registrations.values().all(|w| *w) => {
                    self.terminal = true
                }
                "FAILED" => return Err(ProtocolError("PEER_FAILED")),
                _ => return Err(ProtocolError("CHILD_STATE")),
            }
        }
        Ok(())
    }
    pub fn receive(&mut self, parent: bool) -> Result<bool> {
        self.check()?;
        if self.parent_role.is_some_and(|role| role != parent) {
            return Err(self.poison(ProtocolError("ROLE_CHANGED")));
        }
        self.parent_role = Some(parent);
        if self.eof {
            return Ok(self.cancelled);
        }
        self.last_poll = PollWork::default();
        let result = self.receive_inner(parent);
        result.map_err(|e| self.poison(e))
    }
    fn receive_inner(&mut self, parent: bool) -> Result<bool> {
        loop {
            while self.last_poll.parsed < MAX_PARSE {
                let Some(end) = self.pending.iter().position(|b| *b == b'\n') else {
                    break;
                };
                let line: Vec<_> = self.pending.drain(..=end).collect();
                self.frame(&line[..line.len() - 1], parent)?;
                self.last_poll.parsed += 1;
            }
            if self.terminal && !self.pending.is_empty() {
                return Err(ProtocolError("POST_TERMINAL_DATA"));
            }
            if !self.pending.contains(&b'\n') && self.pending.len() >= MAX_FRAME {
                return Err(ProtocolError("FRAME_LIMIT"));
            }
            if self.last_poll.parsed == MAX_PARSE || self.last_poll.reads == MAX_READS {
                break;
            }
            self.last_poll.reads += 1;
            let mut block = [0u8; MAX_FRAME];
            match self.socket.read(&mut block) {
                Ok(0) => {
                    self.eof = true;
                    if !self.pending.is_empty() {
                        return Err(ProtocolError("TRUNCATED_FRAME"));
                    }
                    if parent {
                        self.cancelled = true;
                        self.owner.borrow_mut().admission_closed = true;
                    } else if self.terminal {
                        self.settled = true;
                    } else {
                        return Err(ProtocolError("EOF_WITHOUT_TERMINAL"));
                    }
                    break;
                }
                Ok(n) => {
                    self.owner.borrow_mut().charge("wire", n)?;
                    if self.pending.len() + n > MAX_PENDING {
                        return Err(ProtocolError("PENDING_LIMIT"));
                    }
                    self.pending.extend_from_slice(&block[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(ProtocolError("READ_FAILED")),
            }
        }
        Ok(self.cancelled)
    }
    pub fn is_settled(&self) -> bool {
        self.settled && self.first_fault.is_none()
    }
    pub fn cancel_deadline_ns(&self) -> Option<u64> {
        self.cancel_deadline_ns
    }
    pub fn take_events(&mut self) -> Vec<String> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod installer_metadata_tests {
    use super::*;
    #[test]
    fn installer_sent_ledger_predicate_is_nonterminal_and_requires_every_wait() {
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut endpoint = Endpoint::new(socket, "ab".repeat(16), None).unwrap();
        assert!(!endpoint.sent_resources_complete());
        endpoint.send_live().unwrap();
        assert!(endpoint.sent_resources_complete());
        let root = std::process::id();
        let child = if root == 42 { 43 } else { 42 };
        endpoint
            .metadata(&format!("REGISTER|{root}|{child}|{}|0", "cd".repeat(16)))
            .unwrap();
        assert!(!endpoint.sent_resources_complete());
        endpoint
            .metadata(&format!("WAITED|{root}|{child}|{}|0", "cd".repeat(16)))
            .unwrap();
        assert!(endpoint.sent_resources_complete());
        assert!(!endpoint.sent_terminal);
        endpoint.send_terminal(true).unwrap();
        assert!(!endpoint.sent_resources_complete());
    }
}

#[cfg(test)]
#[path = "../../tests/unit/source_qa_protocol.rs"]
mod tests;

#[cfg(test)]
mod readiness_metadata_tests {
    use super::*;
    #[test]
    fn child_readiness_hints_do_not_parse_partial_frames_or_accept_budget_yield() {
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut endpoint = Endpoint::new(socket, "ab".repeat(16), None).unwrap();
        let fd = endpoint.socket.as_raw_fd();
        endpoint.pending = b"partial".to_vec();
        assert_eq!(endpoint.progress_wait_source(), Some(fd));
        assert!(!endpoint.progress_buffered_event());
        let pending = endpoint.pending.clone();
        endpoint.last_poll.reads = MAX_READS;
        assert!(endpoint.progress_buffered_event());
        endpoint.last_poll = PollWork {
            reads: 0,
            parsed: MAX_PARSE,
        };
        assert!(endpoint.progress_buffered_event());
        assert_eq!(endpoint.pending, pending);
        assert!(!endpoint.live && !endpoint.terminal && !endpoint.eof && !endpoint.is_settled());
        endpoint.last_poll = PollWork::default();
        endpoint.pending.push(b'\n');
        assert!(endpoint.progress_buffered_event());
        assert!(!endpoint.is_settled());
    }
    #[test]
    fn child_readiness_omits_only_original_receive_eof_even_after_terminal_hup() {
        let (socket, peer_socket) = UnixStream::pair().unwrap();
        let mut endpoint = Endpoint::new(socket, "ab".repeat(16), None).unwrap();
        let mut peer = Endpoint::new(peer_socket, "ab".repeat(16), None).unwrap();
        peer.send_live().unwrap();
        peer.send_terminal(true).unwrap();
        assert!(endpoint.progress_wait_source().is_some());
        assert!(!endpoint.eof && !endpoint.is_settled());
        assert!(!endpoint.receive(false).unwrap());
        assert!(endpoint.eof && endpoint.is_settled());
        endpoint.last_poll = PollWork {
            reads: MAX_READS,
            parsed: MAX_PARSE,
        };
        assert_eq!(endpoint.progress_wait_source(), None);
        assert!(
            !endpoint.progress_buffered_event(),
            "stale cap after EOF would spin on Child wait"
        );
    }
}
