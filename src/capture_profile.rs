//! Diagnostic-only, fixed numeric successful-hook timings. No capture authority.
//! Compiled only with capture-profile; instrumented artifacts are NONPUBLISHABLE.
use std::{
    cell::{Cell, RefCell},
    ffi::CString,
    fmt,
    fmt::Write as _,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path},
};
const STAGES: usize = 26;
const REQUESTED_START: usize = 20;
const REQUESTED_WAITS: usize = 6;
const MAX_RECORD: usize = 1024;
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Phase {
    Pair,
    CoreHash,
    GuardHash,
    Repository,
    Clock,
    StoreOpen,
    EventTransaction,
    Commit,
    OwnerFinish,
    Ack,
    Whole,
    PermitValidation,
    SqliteBegin,
    AuthorityAcquire,
    RefusalReads,
    RepoReadyPoll,
    RepoParkPoll,
    RepoRunningPoll,
    RepoCleaningPoll,
    RepoDonePoll,
    RepoSetupWait,
    RepoRunningWait,
    RepoCleaningWait,
    RepoMixedWait,
    RepoUnknownWait,
    RepoFinishWait,
}
#[derive(Clone)]
struct Record {
    event: u8,
    start: Option<u64>,
    accepted: bool,
    repository_depth: u32,
    valid: u32,
    unknown: u32,
    ns: [u64; STAGES],
    counts: [u32; STAGES],
    requested_ns: [u64; REQUESTED_WAITS],
}
impl Record {
    fn new(event: u8, start: Option<u64>) -> Self {
        Self {
            event,
            start,
            accepted: false,
            repository_depth: 0,
            valid: 0,
            unknown: 0,
            ns: [0; STAGES],
            counts: [0; STAGES],
            requested_ns: [0; REQUESTED_WAITS],
        }
    }
    fn observe(&mut self, phase: Phase, start: Option<u64>, end: Option<u64>) {
        self.observe_requested(phase, start, end, None);
    }
    fn observe_requested(
        &mut self,
        phase: Phase,
        start: Option<u64>,
        end: Option<u64>,
        requested: Option<u64>,
    ) {
        let index = phase as usize;
        let bit = 1u32 << index;
        if self.unknown & bit != 0 {
            return;
        }
        let wait = index
            .checked_sub(REQUESTED_START)
            .filter(|i| *i < REQUESTED_WAITS);
        let requested_sum = match (wait, requested) {
            (Some(i), Some(ns)) => self.requested_ns[i].checked_add(ns),
            (None, None) => Some(0),
            _ => None,
        };
        let elapsed = start.zip(end).and_then(|(a, b)| b.checked_sub(a));
        let sum = elapsed.and_then(|n| self.ns[index].checked_add(n));
        let count = self.counts[index].checked_add(1);
        if let (Some(sum), Some(count), Some(requested_sum)) = (sum, count, requested_sum) {
            self.ns[index] = sum;
            self.counts[index] = count;
            self.valid |= bit;
            if let Some(i) = wait {
                self.requested_ns[i] = requested_sum;
            }
        } else {
            self.valid &= !bit;
            self.unknown |= bit;
            self.ns[index] = 0;
            self.counts[index] = 0;
            if let Some(i) = wait {
                self.requested_ns[i] = 0;
            }
        }
    }
    fn format(&self, limit: usize) -> Result<Buffer, fmt::Error> {
        let mut output = Buffer {
            bytes: [0; MAX_RECORD],
            length: 0,
            limit: limit.min(MAX_RECORD),
        };
        write!(&mut output, "[2,1,{},{}", self.event, self.valid)?;
        for value in self.ns {
            write!(&mut output, ",{value}")?;
        }
        for value in self.counts {
            write!(&mut output, ",{value}")?;
        }
        for value in self.requested_ns {
            write!(&mut output, ",{value}")?;
        }
        output.write_str("]\n")?;
        Ok(output)
    }
}
struct Buffer {
    bytes: [u8; MAX_RECORD],
    length: usize,
    limit: usize,
}
impl fmt::Write for Buffer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self
            .length
            .checked_add(text.len())
            .filter(|n| *n <= self.limit)
            .ok_or(fmt::Error)?;
        self.bytes[self.length..end].copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}
thread_local! {
    static ACTIVE: RefCell<Option<Record>> = const { RefCell::new(None) };
    // Lost numeric observations only; never cancellation/admission authority.
    static REPO_LOST: Cell<bool> = const { Cell::new(false) };
}
const REPO_MASK: u32 = (1 << 3) | (((1 << STAGES) - 1) & !((1 << 15) - 1));
fn repository_phase(phase: Phase) -> bool {
    (phase as usize) >= 15
}
fn lose_repository() {
    let _ = REPO_LOST.try_with(|lost| lost.set(true));
}
fn apply_repository_loss(record: &mut Record) {
    if REPO_LOST
        .try_with(|lost| lost.replace(false))
        .unwrap_or(true)
    {
        record.valid &= !REPO_MASK;
        record.unknown |= REPO_MASK;
        for index in 0..STAGES {
            if REPO_MASK & (1 << index) != 0 {
                record.ns[index] = 0;
                record.counts[index] = 0;
            }
        }
        record.requested_ns = [0; REQUESTED_WAITS];
    }
}
struct RepositoryDepth {
    entered: bool,
}
impl RepositoryDepth {
    fn enter() -> Self {
        let entered = ACTIVE
            .try_with(|active| {
                let Ok(mut active) = active.try_borrow_mut() else {
                    lose_repository();
                    return false;
                };
                let Some(record) = active.as_mut() else {
                    return false;
                };
                apply_repository_loss(record);
                if let Some(next) = record.repository_depth.checked_add(1) {
                    record.repository_depth = next;
                    true
                } else {
                    lose_repository();
                    apply_repository_loss(record);
                    false
                }
            })
            .unwrap_or(false);
        Self { entered }
    }
}
impl Drop for RepositoryDepth {
    fn drop(&mut self) {
        if !self.entered {
            return;
        }
        let _ = ACTIVE.try_with(|active| {
            let Ok(mut active) = active.try_borrow_mut() else {
                lose_repository();
                return;
            };
            if let Some(record) = active.as_mut() {
                if let Some(next) = record.repository_depth.checked_sub(1) {
                    record.repository_depth = next;
                } else {
                    lose_repository();
                }
                apply_repository_loss(record);
            }
        });
    }
}
fn now() -> Option<u64> {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, value.as_mut_ptr()) } != 0 {
        return None;
    }
    let value = unsafe { value.assume_init() };
    let ns = u64::try_from(value.tv_nsec)
        .ok()
        .filter(|n| *n < 1_000_000_000)?;
    u64::try_from(value.tv_sec)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(ns)
}
pub fn begin(event: u8) {
    if option_env!("KEITO_CAPTURE_PROFILE_SINK").is_none() {
        return;
    }
    let start = now();
    let _ = REPO_LOST.try_with(|lost| lost.set(false));
    let _ = ACTIVE.try_with(|active| {
        if let Ok(mut active) = active.try_borrow_mut() {
            *active = (1..=5).contains(&event).then(|| Record::new(event, start));
        }
    });
}
fn keep<T>(value: T, observe: impl FnOnce() -> bool) -> T {
    let _ = observe();
    value
}
pub fn measure<T>(phase: Phase, operation: impl FnOnce() -> T) -> T {
    if option_env!("KEITO_CAPTURE_PROFILE_SINK").is_none() {
        return operation();
    }
    let enabled = ACTIVE
        .try_with(|a| match a.try_borrow() {
            Ok(record) => record.is_some(),
            Err(_) => {
                if matches!(phase, Phase::Repository) {
                    lose_repository();
                }
                false
            }
        })
        .unwrap_or(false);
    if !enabled {
        return operation();
    }
    run_measured(phase, now(), now, operation)
}
fn run_measured<T>(
    phase: Phase,
    start: Option<u64>,
    clock: impl FnOnce() -> Option<u64>,
    operation: impl FnOnce() -> T,
) -> T {
    let depth = matches!(phase, Phase::Repository).then(RepositoryDepth::enter);
    let value = operation();
    drop(depth); // restore also on return/error; RAII restores during unwind
    let end = clock();
    keep(value, || {
        ACTIVE
            .try_with(|active| {
                if let Ok(mut active) = active.try_borrow_mut() {
                    if let Some(record) = active.as_mut() {
                        apply_repository_loss(record);
                        record.observe(phase, start, end);
                        if matches!(phase, Phase::Repository) && record.unknown & REPO_MASK != 0 {
                            lose_repository();
                            apply_repository_loss(record);
                        }
                        return true;
                    }
                }
                if matches!(phase, Phase::Repository) {
                    lose_repository();
                }
                false
            })
            .unwrap_or(false)
    })
}

fn span_enabled(phase: Phase, sink: bool) -> bool {
    sink && ACTIVE
        .try_with(|a| {
            let Ok(mut active) = a.try_borrow_mut() else {
                if repository_phase(phase) {
                    lose_repository();
                }
                return false;
            };
            let Some(record) = active.as_mut() else {
                return false;
            };
            apply_repository_loss(record);
            !repository_phase(phase)
                || (record.repository_depth > 0 && record.unknown & REPO_MASK == 0)
        })
        .unwrap_or(false)
}
/// Numeric observation only. Dropping this span does not finish/cancel/admit
/// any operation or touch the sink. Explicit drop preserves borrowed results.
pub struct Span {
    phase: Phase,
    start: Option<u64>,
    requested: Option<u64>,
    enabled: bool,
}
impl Span {
    pub fn start(phase: Phase) -> Self {
        Self::new(phase, None)
    }
    pub fn wait(phase: Phase, requested_ns: u64) -> Self {
        Self::new(phase, Some(requested_ns))
    }
    fn new(phase: Phase, requested: Option<u64>) -> Self {
        Self::with_clock(
            phase,
            requested,
            option_env!("KEITO_CAPTURE_PROFILE_SINK").is_some(),
            now,
        )
    }
    fn with_clock(
        phase: Phase,
        requested: Option<u64>,
        sink: bool,
        clock: impl FnOnce() -> Option<u64>,
    ) -> Self {
        let enabled = span_enabled(phase, sink);
        Self {
            phase,
            requested,
            enabled,
            start: if enabled { clock() } else { None },
        }
    }
    fn record_at(&self, record: &mut Record, end: Option<u64>) {
        record.observe_requested(self.phase, self.start, end, self.requested);
        // A failed repository observation disables later repository spans.
        // Invalidate earlier partial totals too, so omitted later work cannot
        // be mistaken for a complete, lower-cost stage observation.
        if repository_phase(self.phase) && record.unknown & REPO_MASK != 0 {
            lose_repository();
            apply_repository_loss(record);
        }
    }
}
impl Span {
    fn stop_with_clock(&self, clock: impl FnOnce() -> Option<u64>) {
        if !self.enabled {
            return;
        }
        if repository_phase(self.phase) && !span_enabled(self.phase, true) {
            lose_repository();
            return;
        }
        let end = clock();
        let _ = ACTIVE.try_with(|active| {
            if let Ok(mut active) = active.try_borrow_mut() {
                if let Some(record) = active.as_mut() {
                    apply_repository_loss(record);
                    self.record_at(record, end);
                } else if repository_phase(self.phase) {
                    lose_repository();
                }
            } else if repository_phase(self.phase) {
                lose_repository();
            }
        });
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        self.stop_with_clock(now);
    }
}
pub fn measure_ack<T, E>(accepted: bool, operation: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    let result = measure(Phase::Ack, operation);
    complete(accepted && result.is_ok());
    result
}
pub fn complete(accepted: bool) {
    if option_env!("KEITO_CAPTURE_PROFILE_SINK").is_none() {
        return;
    }
    let end = now();
    let _ = ACTIVE.try_with(|active| {
        if let Ok(mut active) = active.try_borrow_mut() {
            if let Some(record) = active.as_mut() {
                record.accepted = accepted;
                record.observe(Phase::Whole, record.start, end);
            }
        }
    });
}
fn sink_facts(
    mode: libc::mode_t,
    uid: libc::uid_t,
    links: libc::nlink_t,
    own_uid: libc::uid_t,
) -> bool {
    mode & libc::S_IFMT == libc::S_IFREG && mode & 0o7777 == 0o600 && uid == own_uid && links == 1
}
fn write_once(bytes: &[u8], write: impl FnOnce(&[u8]) -> std::io::Result<usize>) -> bool {
    !bytes.is_empty() && bytes.len() <= MAX_RECORD && write(bytes).is_ok_and(|n| n == bytes.len())
}
fn before(end: u64) -> bool {
    now().is_some_and(|at| at < end)
}
fn append(path: &str, record: &Buffer, end: u64) -> Option<()> {
    let path = Path::new(path);
    let raw = path.as_os_str().as_bytes();
    if !path.is_absolute()
        || raw.len() > 4096
        || raw.split(|b| *b == b'/').any(|p| p == b"." || p == b"..")
    {
        return None;
    }
    let count = path
        .components()
        .filter(|p| matches!(p, Component::Normal(_)))
        .count();
    if !(2..=128).contains(&count) {
        return None;
    }
    let parts: Vec<_> = path
        .components()
        .filter_map(|p| {
            if let Component::Normal(p) = p {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    if parts.len() < 2 || parts.len() > 128 || !before(end) {
        return None;
    }
    let fd = unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return None;
    }
    let mut parent = unsafe { OwnedFd::from_raw_fd(fd) };
    let own_uid = unsafe { libc::geteuid() };
    for (index, part) in parts[..parts.len() - 1].iter().enumerate() {
        if !before(end) {
            return None;
        }
        let name = CString::new(part.as_bytes()).ok()?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return None;
        }
        parent = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(parent.as_raw_fd(), info.as_mut_ptr()) } != 0 {
            return None;
        }
        let info = unsafe { info.assume_init() };
        if info.st_uid != 0 && info.st_uid != own_uid {
            return None;
        }
        if index + 2 == parts.len() && (info.st_uid != own_uid || info.st_mode & 0o7777 != 0o700) {
            return None;
        }
    }
    if !before(end) {
        return None;
    }
    let name = CString::new(parts.last()?.as_bytes()).ok()?;
    let mut prior = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            prior.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return None;
    }
    let prior = unsafe { prior.assume_init() };
    if !sink_facts(prior.st_mode, prior.st_uid, prior.st_nlink, own_uid) || !before(end) {
        return None;
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return None;
    }
    let sink = unsafe { File::from_raw_fd(fd) };
    let mut opened = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(sink.as_raw_fd(), opened.as_mut_ptr()) } != 0 {
        return None;
    }
    let info = unsafe { opened.assume_init() };
    if prior.st_dev != info.st_dev
        || prior.st_ino != info.st_ino
        || !sink_facts(info.st_mode, info.st_uid, info.st_nlink, own_uid)
    {
        return None;
    }
    let mut named = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            named.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return None;
    }
    let named = unsafe { named.assume_init() };
    if named.st_dev != info.st_dev
        || named.st_ino != info.st_ino
        || !sink_facts(named.st_mode, named.st_uid, named.st_nlink, own_uid)
        || !before(end)
    {
        return None;
    }
    write_once(&record.bytes[..record.length], |bytes| {
        let count = unsafe { libc::write(sink.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        if count < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    })
    .then_some(())
}
/// Called only after the original main owner finish + terminal publication.
/// Logical result is untouched. File IO is NOT a preemptible wall-clock bound:
/// it can delay child wait and makes this artifact diagnostic NONPUBLISHABLE.
pub fn finish(original_success: bool, end: Option<u64>) {
    let Some(path) = option_env!("KEITO_CAPTURE_PROFILE_SINK") else {
        return;
    };
    let mut record = ACTIVE
        .try_with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.take()))
        .ok()
        .flatten();
    if let Some(record) = record.as_mut() {
        apply_repository_loss(record);
    }
    let Some(record) = record.filter(|r| r.accepted && original_success) else {
        return;
    };
    let Some(end) = end else {
        return;
    };
    if let Ok(bytes) = record.format(MAX_RECORD) {
        let _ = append(path, &bytes, end);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ack_wrapper_preserves_noncopy_results_and_executes_once() {
        struct DropCount<'a>(&'a std::cell::Cell<u32>);
        impl Drop for DropCount<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let called = std::cell::Cell::new(0);
        let dropped = std::cell::Cell::new(0);
        let result = measure_ack(true, || {
            called.set(called.get() + 1);
            Ok::<_, ()>(DropCount(&dropped))
        });
        assert!(result.is_ok());
        assert_eq!(called.get(), 1);
        assert_eq!(dropped.get(), 0);
        drop(result);
        assert_eq!(dropped.get(), 1);
        let result = measure_ack(true, || {
            called.set(called.get() + 1);
            Err::<(), _>(DropCount(&dropped))
        });
        assert!(result.is_err());
        assert_eq!(called.get(), 2);
        assert_eq!(dropped.get(), 1);
        drop(result);
        assert_eq!(dropped.get(), 2);
    }
    #[test]
    fn fixed_numeric_record_round_trips_extremes_without_strings() {
        let mut record = Record::new(2, Some(1));
        record.valid = (1 << STAGES) - 1;
        record.ns = [u64::MAX; STAGES];
        record.counts = [u32::MAX; STAGES];
        record.requested_ns = [u64::MAX; REQUESTED_WAITS];
        let output = record.format(MAX_RECORD).unwrap();
        assert!(output.length <= MAX_RECORD);
        let values: Vec<u64> = serde_json::from_slice(&output.bytes[..output.length]).unwrap();
        assert_eq!(values.len(), 4 + 2 * STAGES + REQUESTED_WAITS);
        assert_eq!(&values[..4], &[2, 1, 2, (1 << STAGES) - 1]);
        assert!(values[4..4 + STAGES].iter().all(|n| *n == u64::MAX));
    }
    #[test]
    fn missing_backwards_and_overflow_clocks_stay_unknown() {
        let mut record = Record::new(1, None);
        record.observe(Phase::Pair, None, Some(10));
        record.observe(Phase::Pair, Some(10), Some(20));
        assert_eq!(record.valid & 1, 0);
        assert_eq!(record.counts[0], 0);
        record.observe(Phase::CoreHash, Some(10), Some(9));
        assert_eq!(record.valid & 2, 0);
        record.ns[2] = u64::MAX;
        record.observe(Phase::GuardHash, Some(0), Some(1));
        assert_eq!(record.valid & 4, 0);
        assert_eq!(record.ns[2], 0);
    }
    #[test]
    fn repeated_phase_accumulates_exact_durations_and_counts() {
        let mut record = Record::new(1, Some(0));
        record.observe(Phase::Commit, Some(1), Some(4));
        record.observe(Phase::Commit, Some(5), Some(10));
        assert_eq!(record.ns[7], 8);
        assert_eq!(record.counts[7], 2);
        assert_ne!(record.valid & (1 << 7), 0);
    }
    #[test]
    fn formatting_failure_and_writer_faults_preserve_original_result() {
        let record = Record::new(1, Some(0));
        assert!(record.format(1).is_err());
        let original: Result<u8, u8> = Err(17);
        assert_eq!(keep(original, || false), Err(17));
        let count = std::cell::Cell::new(0);
        assert!(!write_once(b"[1]\n", |_| {
            count.set(count.get() + 1);
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
        }));
        assert_eq!(count.get(), 1);
        assert!(!write_once(b"[1]\n", |_| Ok(2)));
        assert!(!write_once(&[0; MAX_RECORD + 1], |_| panic!(
            "oversize writer invoked"
        )));
        assert!(write_once(b"[1]\n", |b| Ok(b.len())));
    }
    #[test]
    fn sink_metadata_refuses_wrong_owner_mode_links_and_nonregular_files() {
        let regular = libc::S_IFREG | 0o600;
        assert!(sink_facts(regular, 42, 1, 42));
        assert!(!sink_facts(regular, 41, 1, 42));
        assert!(!sink_facts(regular, 42, 2, 42));
        assert!(!sink_facts(libc::S_IFREG | 0o644, 42, 1, 42));
        assert!(!sink_facts(libc::S_IFREG | 0o4600, 42, 1, 42));
        assert!(!sink_facts(libc::S_IFLNK | 0o600, 42, 1, 42));
    }
    #[test]
    fn span_records_borrow_safe_single_interval_without_io() {
        let mut record = Record::new(1, Some(0));
        let span = Span {
            phase: Phase::SqliteBegin,
            start: Some(10),
            requested: None,
            enabled: false,
        };
        span.record_at(&mut record, Some(25));
        assert_eq!(record.ns[Phase::SqliteBegin as usize], 15);
        assert_eq!(record.counts[Phase::SqliteBegin as usize], 1);
    }
    #[test]
    fn requested_waits_accumulate_and_overflow_or_missing_value_stays_unknown() {
        let mut record = Record::new(1, Some(0));
        record.observe_requested(Phase::RepoSetupWait, Some(1), Some(4), Some(10));
        record.observe_requested(Phase::RepoSetupWait, Some(5), Some(8), Some(20));
        assert_eq!(record.requested_ns[0], 30);
        assert_eq!(record.ns[20], 6);
        assert_eq!(record.counts[20], 2);
        record.requested_ns[0] = u64::MAX;
        record.observe_requested(Phase::RepoSetupWait, Some(9), Some(10), Some(1));
        assert_eq!(record.valid & (1 << 20), 0);
        assert_eq!(record.requested_ns[0], 0);
        record.observe_requested(Phase::RepoSetupWait, Some(11), Some(12), Some(1));
        assert_eq!(record.counts[20], 0);
        record.observe(Phase::RepoRunningWait, Some(1), Some(2));
        assert_eq!(record.valid & (1 << 21), 0);
    }
    #[test]
    fn repository_clock_loss_invalidates_partial_totals_before_later_spans_are_skipped() {
        with_numeric_record(|| {
            run_measured(
                Phase::Repository,
                Some(0),
                || Some(10),
                || {
                    let earlier = Span::with_clock(Phase::RepoReadyPoll, None, true, || Some(1));
                    earlier.stop_with_clock(|| Some(2));
                    std::mem::forget(earlier);
                    let failed = Span::with_clock(Phase::RepoParkPoll, None, true, || Some(3));
                    failed.stop_with_clock(|| None);
                    std::mem::forget(failed);
                    let later = Span::with_clock(Phase::RepoCleaningPoll, None, true, || {
                        panic!("invalid repository metrics must not read another clock")
                    });
                    assert!(!later.enabled);
                },
            );
            ACTIVE.with(|active| {
                let active = active.borrow();
                let record = active.as_ref().unwrap();
                assert_eq!(record.valid & REPO_MASK, 0);
                assert_eq!(record.unknown & REPO_MASK, REPO_MASK);
                assert_eq!(record.counts[Phase::RepoReadyPoll as usize], 0);
                assert_eq!(record.ns[Phase::RepoReadyPoll as usize], 0);
                assert_eq!(record.requested_ns, [0; REQUESTED_WAITS]);
            });
        });
    }
    #[test]
    fn schema_two_keeps_phase_prefix_and_fixed_requested_tail() {
        assert_eq!(Phase::Whole as usize, 10);
        assert_eq!(Phase::PermitValidation as usize, 11);
        assert_eq!(Phase::RefusalReads as usize, 14);
        assert_eq!(Phase::RepoReadyPoll as usize, 15);
        assert_eq!(Phase::RepoFinishWait as usize, STAGES - 1);
        let mut record = Record::new(2, Some(0));
        record.observe_requested(Phase::RepoFinishWait, Some(1), Some(3), Some(7));
        let row = record.format(MAX_RECORD).unwrap();
        let values: Vec<u64> = serde_json::from_slice(&row.bytes[..row.length]).unwrap();
        assert_eq!(values.len(), 62);
        assert_eq!(values[0], 2);
        assert_eq!(values[61], 7);
    }

    fn with_numeric_record(operation: impl FnOnce()) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                let _ = ACTIVE.try_with(|a| {
                    if let Ok(mut a) = a.try_borrow_mut() {
                        *a = None;
                    }
                });
                let _ = REPO_LOST.try_with(|lost| lost.set(false));
            }
        }
        ACTIVE.with(|a| *a.borrow_mut() = Some(Record::new(1, Some(0))));
        REPO_LOST.with(|lost| lost.set(false));
        let _reset = Reset;
        operation();
    }
    fn depth() -> u32 {
        ACTIVE.with(|a| a.borrow().as_ref().unwrap().repository_depth)
    }
    #[test]
    fn repository_depth_restores_normal_error_nested_and_unwind_paths() {
        with_numeric_record(|| {
            assert_eq!(depth(), 0);
            let result = run_measured(
                Phase::Repository,
                Some(1),
                || Some(10),
                || {
                    assert_eq!(depth(), 1);
                    run_measured(
                        Phase::Repository,
                        Some(2),
                        || Some(5),
                        || assert_eq!(depth(), 2),
                    );
                    assert_eq!(depth(), 1);
                    Err::<(), u8>(7)
                },
            );
            assert_eq!(result, Err(7));
            assert_eq!(depth(), 0);
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_measured(
                    Phase::Repository,
                    Some(1),
                    || Some(2),
                    || panic!("synthetic unwind"),
                )
            }));
            assert!(panic.is_err());
            assert_eq!(depth(), 0);
        });
    }
    #[test]
    fn repository_spans_need_sink_active_record_and_repository_depth_before_clock() {
        ACTIVE.with(|a| *a.borrow_mut() = None);
        REPO_LOST.with(|lost| lost.set(false));
        let clocks = Cell::new(0);
        let tick = || {
            clocks.set(clocks.get() + 1);
            Some(1)
        };
        let inactive = Span::with_clock(Phase::RepoReadyPoll, None, false, tick);
        assert!(!inactive.enabled);
        assert_eq!(clocks.get(), 0);
        let no_record = Span::with_clock(Phase::RepoReadyPoll, None, true, tick);
        assert!(!no_record.enabled);
        assert_eq!(clocks.get(), 0);
        with_numeric_record(|| {
            let outside = Span::with_clock(Phase::RepoReadyPoll, None, true, tick);
            assert!(!outside.enabled);
            assert_eq!(clocks.get(), 0);
            run_measured(
                Phase::Repository,
                Some(0),
                || Some(2),
                || {
                    let span = Span::with_clock(Phase::RepoReadyPoll, None, true, tick);
                    assert!(span.enabled);
                    assert_eq!(clocks.get(), 1);
                    std::mem::forget(span); // injected-clock gating test; avoid actual Drop clock
                },
            );
        });
    }
    #[test]
    fn escaped_repository_span_stops_without_clock_and_is_unknown() {
        with_numeric_record(|| {
            let span = run_measured(
                Phase::Repository,
                Some(0),
                || Some(3),
                || Span::with_clock(Phase::RepoReadyPoll, None, true, || Some(1)),
            );
            assert_eq!(depth(), 0);
            let clocks = Cell::new(0);
            span.stop_with_clock(|| {
                clocks.set(1);
                Some(5)
            });
            assert_eq!(clocks.get(), 0);
            ACTIVE.with(|a| {
                let mut a = a.borrow_mut();
                let record = a.as_mut().unwrap();
                apply_repository_loss(record);
                assert_eq!(record.valid & REPO_MASK, 0);
            });
        });
    }
    #[test]
    fn repository_overflow_and_borrow_loss_invalidate_instead_of_faking_zero_cost() {
        with_numeric_record(|| {
            ACTIVE.with(|a| a.borrow_mut().as_mut().unwrap().repository_depth = u32::MAX);
            let _depth = RepositoryDepth::enter();
            ACTIVE
                .with(|a| assert_eq!(a.borrow().as_ref().unwrap().unknown & REPO_MASK, REPO_MASK));
        });
        with_numeric_record(|| {
            ACTIVE.with(|a| {
                let mut held = a.borrow_mut();
                let record = held.as_mut().unwrap();
                record.valid |= 1 << 15;
                record.ns[15] = 5;
                record.counts[15] = 1;
                let clocks = Cell::new(0);
                let span = Span::with_clock(Phase::RepoReadyPoll, None, true, || {
                    clocks.set(1);
                    Some(1)
                });
                assert!(!span.enabled);
                assert_eq!(clocks.get(), 0);
            });
            ACTIVE.with(|a| {
                let mut a = a.borrow_mut();
                let record = a.as_mut().unwrap();
                apply_repository_loss(record);
                assert_eq!(record.unknown & REPO_MASK, REPO_MASK);
                assert_eq!(record.valid & REPO_MASK, 0);
                assert_eq!(record.ns[15], 0);
                assert_eq!(record.counts[15], 0);
            });
        });
    }
}
