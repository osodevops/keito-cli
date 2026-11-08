//! Bounded active-session readers; normal numeric extraction remains gated.
//!
//! No consent, hook installation, watcher, raw persistence, complete-coverage
//! claim, pricing or expense creation. Supported numeric shapes are based on
//! minimized probes; stream/interruption/finality qualification remains pending.
//! Consent baselines use identity-only decoding, not historical numeric evidence.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 4096;
const MAX_COUNTER: u64 = 1_000_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harness {
    ClaudeCode,
    Codex,
}
impl Harness {
    fn version(self) -> &'static str {
        match self {
            Self::ClaudeCode => "2.1.319",
            Self::Codex => "0.167.0",
        }
    }
    fn adapter(self) -> &'static str {
        match self {
            Self::ClaudeCode => "CLAUDE_JSONL_2_1_319_V1",
            Self::Codex => "CODEX_ROLLOUT_0_167_0_V1",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageReadError {
    UnsupportedVersion,
    UnsafePath,
    InvalidSession,
    Unavailable,
    TooLarge,
    InvalidRecord,
    SourceChanged,
}
impl std::fmt::Display for UsageReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Active-session usage unavailable ({self:?}).")
    }
}
impl std::error::Error for UsageReadError {}

// Deliberately no Debug/Serialize: the path is a transient capability, never an
// evidence or diagnostic field. The caller owns consent and active-session proof.
pub struct ApprovedActiveSession {
    harness: Harness,
    session_id: String,
    path: PathBuf,
}
impl ApprovedActiveSession {
    /// `resolved_home` must come from the consent-owning core, never hook input.
    /// A hook path alone is not authorization. Only harness-managed roots qualify.
    /// No environment/config is read and no path is searched or canonicalized.
    pub fn new(
        harness: Harness,
        version: &str,
        resolved_home: &Path,
        path: &Path,
        session_id: &str,
    ) -> Result<Self, UsageReadError> {
        if version != harness.version() {
            return Err(UsageReadError::UnsupportedVersion);
        }
        if !native_session_id(session_id) {
            return Err(UsageReadError::InvalidSession);
        }
        if !absolute_clean(resolved_home)
            || resolved_home == Path::new("/")
            || !absolute_clean(path)
        {
            return Err(UsageReadError::UnsafePath);
        }
        let root = resolved_home.join(match harness {
            Harness::ClaudeCode => ".claude/projects",
            Harness::Codex => ".codex/sessions",
        });
        let relative = path
            .strip_prefix(root)
            .map_err(|_| UsageReadError::UnsafePath)?;
        let parts: Vec<_> = relative.components().collect();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(UsageReadError::UnsafePath)?;
        let allowed = match harness {
            Harness::ClaudeCode => parts.len() == 2 && filename == format!("{session_id}.jsonl"),
            Harness::Codex => {
                parts.len() == 4
                    && parts[..3].iter().zip([4, 2, 2]).all(|(p, n)| {
                        p.as_os_str()
                            .to_str()
                            .is_some_and(|s| s.len() == n && s.bytes().all(|b| b.is_ascii_digit()))
                    })
                    && filename.starts_with("rollout-")
                    && filename.ends_with(&format!("-{session_id}.jsonl"))
            }
        };
        if !allowed {
            return Err(UsageReadError::UnsafePath);
        }
        Ok(Self {
            harness,
            session_id: session_id.to_owned(),
            path: path.to_owned(),
        })
    }
}
fn native_session_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn absolute_clean(path: &Path) -> bool {
    // `components` normalizes interior '.', so inspect raw spelling as well.
    path.is_absolute()
        && path
            .as_os_str()
            .to_str()
            .is_some_and(|s| !s.split('/').any(|p| p == "." || p == ".."))
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RequestUsage {
    pub source_adapter: &'static str,
    pub source_record_id: String,
    pub source_turn_id: Option<String>,
    pub model: Option<String>,
    pub model_provenance: &'static str,
    pub counter_semantics: &'static str,
    pub input_basis: &'static str,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct UsageEvidence {
    pub source_session_id: String,
    pub harness_version: &'static str,
    pub coverage: &'static str,
    pub coverage_reason: &'static str,
    pub requests: Vec<RequestUsage>,
    pub limitations: Vec<&'static str>,
}

/// Consent baselines retain source identities only, never historical counters,
/// model names, transcript paths, content or provisional usage snapshots.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ObservedRequestIds {
    pub source_session_id: String,
    pub harness_version: &'static str,
    pub source_adapter: &'static str,
    pub record_ids: BTreeSet<String>,
    pub limitations: Vec<&'static str>,
}

pub fn read_active_session_request_ids(
    approved: &ApprovedActiveSession,
) -> Result<ObservedRequestIds, UsageReadError> {
    let bytes = bounded_snapshot(&approved.path)?;
    let (lines, incomplete_tail) = complete_lines(&bytes)?;
    let mut record_ids = BTreeSet::new();
    let mut saw_meta = false;
    for line in lines {
        let header: Header = decode(line)?;
        match (approved.harness, header.kind.as_str()) {
            (Harness::ClaudeCode, "assistant") => {
                record_ids.insert(claude_request_id(approved, line)?);
            }
            (Harness::Codex, "session_meta") => {
                verify_session_meta(approved, line)?;
                saw_meta = true;
            }
            (Harness::Codex, "token_usage_record") => {
                record_ids.insert(codex_request_id(approved, line, saw_meta)?);
            }
            _ => (),
        }
    }
    if approved.harness == Harness::Codex && !saw_meta {
        return Err(UsageReadError::InvalidSession);
    }
    Ok(ObservedRequestIds {
        source_session_id: approved.session_id.clone(),
        harness_version: approved.harness.version(),
        source_adapter: approved.harness.adapter(),
        record_ids,
        limitations: if incomplete_tail {
            vec!["INCOMPLETE_TAIL"]
        } else {
            vec![]
        },
    })
}

pub fn read_active_session_excluding_request_ids(
    approved: &ApprovedActiveSession,
    excluded: &BTreeSet<String>,
) -> Result<UsageEvidence, UsageReadError> {
    let bytes = bounded_snapshot(&approved.path)?;
    parse_snapshot(approved, &bytes, excluded)
}

pub fn read_active_session(
    approved: &ApprovedActiveSession,
) -> Result<UsageEvidence, UsageReadError> {
    read_active_session_excluding_request_ids(approved, &BTreeSet::new())
}
#[cfg(unix)]
fn bounded_snapshot(path: &Path) -> Result<Vec<u8>, UsageReadError> {
    use std::ffi::CString;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    // Pin every directory by descriptor. No stat-then-open window, symlink
    // traversal, FIFO blocking, recursive search or fallback filename.
    let name = CString::new("/").map_err(|_| UsageReadError::UnsafePath)?;
    let root_fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(UsageReadError::Unavailable);
    }
    let mut parent = unsafe { OwnedFd::from_raw_fd(root_fd) };
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| {
            if let Component::Normal(v) = c {
                Some(v)
            } else {
                None
            }
        })
        .collect();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(part.as_bytes()).map_err(|_| UsageReadError::UnsafePath)?;
        let last = index + 1 == parts.len();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if last { 0 } else { libc::O_DIRECTORY };
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(UsageReadError::UnsafePath);
        }
        parent = unsafe { OwnedFd::from_raw_fd(fd) };
    }
    let mut file = std::fs::File::from(parent);
    let before = file.metadata().map_err(|_| UsageReadError::Unavailable)?;
    if !before.is_file() || before.nlink() != 1 || before.uid() != unsafe { libc::geteuid() } {
        return Err(UsageReadError::UnsafePath);
    }
    if before.len() > MAX_SNAPSHOT_BYTES as u64 {
        return Err(UsageReadError::TooLarge);
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.by_ref()
        .take((MAX_SNAPSHOT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| UsageReadError::Unavailable)?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(UsageReadError::TooLarge);
    }
    let after = file.metadata().map_err(|_| UsageReadError::Unavailable)?;
    if bytes.len() as u64 != before.len()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(UsageReadError::SourceChanged);
    }
    Ok(bytes)
}
#[cfg(not(unix))]
fn bounded_snapshot(_path: &Path) -> Result<Vec<u8>, UsageReadError> {
    Err(UsageReadError::Unavailable)
}

#[derive(Deserialize)]
struct Header {
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct ClaudeIdentity {
    #[serde(rename = "sessionId")]
    session_id: String,
}
#[derive(Deserialize)]
struct ClaudeObservedRecord {
    message: ClaudeObservedMessage,
}
#[derive(Deserialize)]
struct ClaudeObservedMessage {
    id: String,
}
#[derive(Deserialize)]
struct ClaudeRecord {
    #[serde(rename = "sessionId")]
    session_id: String,
    message: ClaudeMessage,
}
#[derive(Deserialize)]
struct ClaudeMessage {
    id: String,
    model: Option<String>,
    usage: Option<ClaudeTokens>,
    stop_reason: Option<ClaudeStopReason>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClaudeStopReason {
    ToolUse,
    EndTurn,
    #[serde(other)]
    Unqualified,
}
#[derive(Deserialize)]
struct ClaudeTokens {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}
#[derive(Deserialize)]
struct Envelope<T> {
    payload: T,
}
#[derive(Deserialize)]
struct SessionMeta {
    id: String,
    session_id: String,
    cli_version: Option<String>,
}
#[derive(Deserialize)]
struct TurnContext {
    turn_id: Option<String>,
    model: Option<String>,
    session_id: Option<String>,
    thread_id: Option<String>,
}
#[derive(Deserialize)]
struct LifecycleEvent {
    #[serde(rename = "type")]
    kind: String,
    turn_id: Option<String>,
    session_id: Option<String>,
    thread_id: Option<String>,
}
#[derive(Deserialize)]
struct CodexRecord {
    thread_id: String,
    session_id: String,
    turn_id: String,
    root_turn_id: String,
    response_id: String,
    usage: Option<CodexTokens>,
}
#[derive(Deserialize)]
struct CodexObservedRecord {
    thread_id: String,
    session_id: String,
    turn_id: String,
    root_turn_id: String,
    response_id: String,
}
#[derive(Deserialize)]
struct CodexTokens {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
}
fn decode<'a, T: Deserialize<'a>>(line: &'a [u8]) -> Result<T, UsageReadError> {
    // IgnoredAny can skip deeper than serde's ordinary value recursion limit.
    // Budget nesting even in fields we discard, without materializing prose.
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for byte in line {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 64 {
                        return Err(UsageReadError::InvalidRecord);
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => (),
            }
        }
    }
    // Typed decoding ignores prose without retaining values; duplicate known
    // fields fail closed. Neither JSON decoder errors nor input are exposed.
    serde_json::from_slice(line).map_err(|_| UsageReadError::InvalidRecord)
}
fn opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        && ![
            "kto_",
            "sk-",
            "sk_",
            "akia",
            "asia",
            "ghp_",
            "gho_",
            "ghu_",
            "ghs_",
            "ghr_",
            "github_pat_",
            "xoxb-",
            "xoxp-",
        ]
        .iter()
        .any(|p| value.to_ascii_lowercase().starts_with(p))
}
fn identifier(value: &str) -> Result<(), UsageReadError> {
    if opaque(value) {
        Ok(())
    } else {
        Err(UsageReadError::InvalidRecord)
    }
}
fn model(value: Option<String>) -> Result<Option<String>, UsageReadError> {
    if let Some(ref v) = value {
        identifier(v)?;
    }
    Ok(value)
}
fn complete_lines(bytes: &[u8]) -> Result<(Vec<&[u8]>, bool), UsageReadError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(UsageReadError::TooLarge);
    }
    let mut lines = Vec::new();
    let mut count = 0;
    let mut tail = false;
    for chunk in bytes.split_inclusive(|b| *b == b'\n') {
        if chunk.last() != Some(&b'\n') {
            tail = true;
            break;
        }
        let line = &chunk[..chunk.len() - 1];
        count += 1;
        if count > MAX_RECORDS || line.len() > MAX_LINE_BYTES {
            return Err(UsageReadError::TooLarge);
        }
        if !line.iter().all(u8::is_ascii_whitespace) {
            lines.push(line);
        }
    }
    Ok((lines, tail))
}
fn claude_request_id(
    approved: &ApprovedActiveSession,
    line: &[u8],
) -> Result<String, UsageReadError> {
    let identity: ClaudeIdentity = decode(line).map_err(|_| UsageReadError::InvalidSession)?;
    if identity.session_id != approved.session_id {
        return Err(UsageReadError::InvalidSession);
    }
    let record: ClaudeObservedRecord = decode(line)?;
    identifier(&record.message.id)?;
    if !record.message.id.starts_with("msg_") {
        return Err(UsageReadError::InvalidRecord);
    }
    Ok(record.message.id)
}
fn verify_session_meta(
    approved: &ApprovedActiveSession,
    line: &[u8],
) -> Result<(), UsageReadError> {
    let meta: Envelope<SessionMeta> = decode(line).map_err(|_| UsageReadError::InvalidSession)?;
    if meta.payload.id != approved.session_id || meta.payload.session_id != approved.session_id {
        return Err(UsageReadError::InvalidSession);
    }
    if meta.payload.cli_version.as_deref() != Some(approved.harness.version()) {
        return Err(UsageReadError::UnsupportedVersion);
    }
    Ok(())
}
fn codex_request_id(
    approved: &ApprovedActiveSession,
    line: &[u8],
    saw_meta: bool,
) -> Result<String, UsageReadError> {
    let raw: Envelope<CodexObservedRecord> = decode(line)?;
    let raw = raw.payload;
    if !saw_meta || raw.session_id != approved.session_id || raw.thread_id != approved.session_id {
        return Err(UsageReadError::InvalidSession);
    }
    identifier(&raw.response_id)?;
    identifier(&raw.turn_id)?;
    identifier(&raw.root_turn_id)?;
    if !raw.response_id.starts_with("resp_") {
        return Err(UsageReadError::InvalidRecord);
    }
    Ok(raw.response_id)
}
fn validate_counters(request: &RequestUsage) -> Result<(), UsageReadError> {
    if [
        request.input_tokens,
        request.cached_input_tokens,
        request.cache_write_input_tokens,
        request.output_tokens,
        request.reasoning_output_tokens,
    ]
    .into_iter()
    .flatten()
    .any(|n| n > MAX_COUNTER)
    {
        return Err(UsageReadError::InvalidRecord);
    }
    if request
        .reasoning_output_tokens
        .zip(request.output_tokens)
        .is_some_and(|(reasoning, output)| reasoning > output)
        || (request.input_basis == "CODEX_TOTAL_INPUT"
            && request.input_tokens.is_some_and(|input| {
                request.cached_input_tokens.unwrap_or(0)
                    + request.cache_write_input_tokens.unwrap_or(0)
                    > input
            }))
    {
        return Err(UsageReadError::InvalidRecord);
    }
    // Claude cache categories are additional input; normalization belongs to
    // the server. Codex cached input and reasoning remain their raw subsets.
    Ok(())
}
fn parse_snapshot(
    approved: &ApprovedActiveSession,
    bytes: &[u8],
    excluded: &BTreeSet<String>,
) -> Result<UsageEvidence, UsageReadError> {
    let mut limitations = BTreeSet::new();
    let mut contexts: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut candidates = Vec::new();
    let mut held_requests = BTreeSet::new();
    let mut provisional_requests = BTreeSet::new();
    let mut final_requests = BTreeSet::new();
    let mut started_turns = BTreeSet::new();
    let mut completed_turns = BTreeSet::new();
    let mut interrupted_turns = BTreeSet::new();
    let mut saw_meta = false;
    let mut records = 0;
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let line = &bytes[start..index];
        start = index + 1;
        if line.len() > MAX_LINE_BYTES {
            return Err(UsageReadError::TooLarge);
        }
        records += 1;
        if records > MAX_RECORDS {
            return Err(UsageReadError::TooLarge);
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let header: Header = decode(line)?;
        match (approved.harness, header.kind.as_str()) {
            (Harness::ClaudeCode, "assistant") => {
                let id = claude_request_id(approved, line)?;
                // A consent baseline excludes historical IDs before decoding
                // their numeric, model or finality fields. They never become
                // numeric evidence or conflicts in this authorized epoch.
                if excluded.contains(&id) {
                    continue;
                }
                // A filename is not identity. Every usage-bearing record must
                // identify this exact approved session.
                let identity: ClaudeIdentity =
                    decode(line).map_err(|_| UsageReadError::InvalidSession)?;
                if identity.session_id != approved.session_id {
                    return Err(UsageReadError::InvalidSession);
                }
                let raw: ClaudeRecord = decode(line)?;
                if raw.session_id != approved.session_id {
                    return Err(UsageReadError::InvalidSession);
                }
                identifier(&raw.message.id)?;
                if !raw.message.id.starts_with("msg_") {
                    return Err(UsageReadError::InvalidRecord);
                }
                // Provisional stream counters are cumulative snapshots, not
                // deltas. Ignore them, then consume a qualified final once.
                // Actual same-ID stream delivery remains a qualification gate;
                // the policy follows the primary final-usage contract.
                match raw.message.stop_reason {
                    None => {
                        provisional_requests.insert(raw.message.id);
                        continue;
                    }
                    Some(ClaudeStopReason::Unqualified) => {
                        held_requests.insert(raw.message.id);
                        limitations.insert("UNKNOWN_REQUEST_FINALITY");
                        continue;
                    }
                    Some(ClaudeStopReason::ToolUse | ClaudeStopReason::EndTurn) => {
                        final_requests.insert(raw.message.id.clone());
                    }
                }
                let usage = raw.message.usage;
                if usage.is_none() {
                    limitations.insert("MISSING_USAGE");
                }
                let model = model(raw.message.model)?;
                if model.is_none() {
                    limitations.insert("MISSING_MODEL");
                }
                candidates.push(RequestUsage {
                    source_adapter: approved.harness.adapter(),
                    source_record_id: raw.message.id,
                    source_turn_id: None,
                    model_provenance: if model.is_some() {
                        "RESPONSE_MODEL"
                    } else {
                        "UNKNOWN"
                    },
                    model,
                    counter_semantics: "REQUEST_DELTA",
                    input_basis: "CLAUDE_UNCACHED_INPUT",
                    input_tokens: usage.as_ref().and_then(|u| u.input_tokens),
                    cached_input_tokens: usage.as_ref().and_then(|u| u.cache_read_input_tokens),
                    cache_write_input_tokens: usage
                        .as_ref()
                        .and_then(|u| u.cache_creation_input_tokens),
                    output_tokens: usage.as_ref().and_then(|u| u.output_tokens),
                    reasoning_output_tokens: None,
                });
            }
            (Harness::Codex, "session_meta") => {
                verify_session_meta(approved, line)?;
                saw_meta = true;
            }
            (Harness::Codex, "event_msg") => {
                let event: Envelope<LifecycleEvent> = decode(line)?;
                let event = event.payload;
                if event.session_id.is_some_and(|id| id != approved.session_id)
                    || event.thread_id.is_some_and(|id| id != approved.session_id)
                {
                    return Err(UsageReadError::InvalidSession);
                }
                if ["task_started", "task_complete", "turn_aborted"].contains(&event.kind.as_str())
                {
                    let turn = event.turn_id.ok_or(UsageReadError::InvalidRecord)?;
                    identifier(&turn)?;
                    match event.kind.as_str() {
                        "task_started" => {
                            started_turns.insert(turn);
                        }
                        "task_complete" => {
                            completed_turns.insert(turn);
                        }
                        "turn_aborted" => {
                            interrupted_turns.insert(turn);
                        }
                        _ => (),
                    }
                }
            }
            (Harness::Codex, "turn_context") => {
                let raw: Envelope<TurnContext> = decode(line)?;
                if raw
                    .payload
                    .session_id
                    .is_some_and(|id| id != approved.session_id)
                    || raw
                        .payload
                        .thread_id
                        .is_some_and(|id| id != approved.session_id)
                {
                    return Err(UsageReadError::InvalidSession);
                }
                if let Some(turn) = raw.payload.turn_id {
                    identifier(&turn)?;
                    let value = model(raw.payload.model)?;
                    // Conflicting same-turn model snapshots mean unknown
                    // attribution, never last-model-wins after compaction.
                    if let Some(previous) = contexts.get(&turn) {
                        if previous != &value {
                            contexts.insert(turn, None);
                            limitations.insert("CONFLICTING_TURN_MODELS");
                        }
                    } else {
                        contexts.insert(turn, value);
                    }
                }
            }
            (Harness::Codex, "token_usage_record") => {
                let id = codex_request_id(approved, line, saw_meta)?;
                if excluded.contains(&id) {
                    continue;
                }
                let raw: Envelope<CodexRecord> = decode(line)?;
                let raw = raw.payload;
                if !saw_meta
                    || raw.session_id != approved.session_id
                    || raw.thread_id != approved.session_id
                {
                    return Err(UsageReadError::InvalidSession);
                }
                identifier(&raw.response_id)?;
                identifier(&raw.turn_id)?;
                identifier(&raw.root_turn_id)?;
                if !raw.response_id.starts_with("resp_") {
                    return Err(UsageReadError::InvalidRecord);
                }
                let usage = raw.usage;
                if usage.is_none() {
                    limitations.insert("MISSING_USAGE");
                }
                candidates.push(RequestUsage {
                    source_adapter: approved.harness.adapter(),
                    source_record_id: raw.response_id,
                    source_turn_id: Some(raw.turn_id),
                    model: None,
                    model_provenance: "UNKNOWN",
                    counter_semantics: "REQUEST_DELTA",
                    input_basis: "CODEX_TOTAL_INPUT",
                    input_tokens: usage.as_ref().and_then(|u| u.input_tokens),
                    cached_input_tokens: usage.as_ref().and_then(|u| u.cached_input_tokens),
                    cache_write_input_tokens: usage
                        .as_ref()
                        .and_then(|u| u.cache_write_input_tokens),
                    output_tokens: usage.as_ref().and_then(|u| u.output_tokens),
                    reasoning_output_tokens: usage.as_ref().and_then(|u| u.reasoning_output_tokens),
                });
            }
            _ => (),
        }
    }
    if start < bytes.len() {
        limitations.insert("INCOMPLETE_TAIL");
    }
    if approved.harness == Harness::Codex && !saw_meta {
        return Err(UsageReadError::InvalidSession);
    }
    if provisional_requests
        .difference(&final_requests)
        .next()
        .is_some()
    {
        // This is resolvable incomplete coverage, not a permanent conflict.
        limitations.insert("PENDING_REQUEST_FINALITY");
    }
    let mut requests: BTreeMap<String, Option<RequestUsage>> = BTreeMap::new();
    for mut request in candidates {
        if let Some(turn) = &request.source_turn_id {
            request.model = contexts.get(turn).and_then(Clone::clone);
            if request.model.is_some() {
                request.model_provenance = "TURN_CONFIG";
            } else {
                limitations.insert("MISSING_MODEL");
            }
        }
        validate_counters(&request)?;
        if request.input_tokens.is_none() && request.output_tokens.is_none() {
            limitations.insert("MISSING_USAGE");
        }
        match requests.get(&request.source_record_id) {
            None => {
                requests.insert(request.source_record_id.clone(), Some(request));
            }
            Some(Some(previous)) if previous == &request => (),
            _ => {
                limitations.insert("CONFLICTING_REQUEST_RECORDS");
                requests.insert(request.source_record_id.clone(), None);
            }
        }
    }
    let requests: Vec<_> = requests
        .into_values()
        .flatten()
        .filter(|r| !held_requests.contains(&r.source_record_id))
        .filter(|r| r.input_tokens.is_some() || r.output_tokens.is_some())
        .collect();
    let observed_turns: BTreeSet<_> = requests
        .iter()
        .filter_map(|r| r.source_turn_id.as_ref())
        .collect();
    if !interrupted_turns.is_empty() {
        limitations.insert("INTERRUPTED_TURN");
    }
    if started_turns
        .iter()
        .any(|turn| !completed_turns.contains(turn) && !interrupted_turns.contains(turn))
    {
        limitations.insert("IN_PROGRESS_TURN");
    }
    if started_turns
        .iter()
        .chain(completed_turns.iter())
        .chain(interrupted_turns.iter())
        .any(|turn| !observed_turns.contains(turn))
    {
        // The aborted probe emitted no usage record. Absence cannot establish
        // zero provider consumption, even after an explicit terminal event.
        limitations.insert("MISSING_TURN_USAGE");
    }
    if requests.is_empty() {
        limitations.insert("NO_OBSERVED_REQUEST_RECORDS");
    }
    Ok(UsageEvidence {
        source_session_id: approved.session_id.clone(),
        harness_version: approved.harness.version(),
        coverage: "PARTIAL",
        coverage_reason: "OBSERVED_REQUEST_RECORDS",
        requests,
        limitations: limitations.into_iter().collect(),
    })
}
