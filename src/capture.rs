//! Metadata-only durable capture. Raw hook JSON is never a storage type.
use crate::cli::capture::CaptureEvent;
use crate::error::AppError;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const MAX_EVENT_BYTES: usize = 1024 * 1024;
pub const MAX_BODY_BYTES: usize = 16 * 1024;
const MAX_QUEUE_BYTES: i64 = 100 * 1024 * 1024;
const MAX_INTERVAL_SECONDS: i64 = 24 * 60 * 60;
const POLICY: &str = "METADATA_ONLY_OBSERVED_TURNS_V1";

fn storage_error(_: impl std::fmt::Display) -> AppError {
    AppError::Config("Capture storage unavailable; inspect disk space and permissions. Evidence was not discarded.".into())
}
pub fn hash(value: impl AsRef<[u8]>) -> String {
    Sha256::digest(value.as_ref())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub fn identifier(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || value.contains("..")
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        || [
            "kto_",
            "sk-",
            "sk_",
            "AKIA",
            "ghp_",
            "gho_",
            "github_pat_",
            "xoxb-",
            "xoxp-",
        ]
        .iter()
        .any(|p| value.starts_with(p))
    {
        return Err(AppError::InvalidInput(
            "Capture identifier rejected; use an opaque identifier without paths or credentials"
                .into(),
        ));
    }
    Ok(())
}
pub fn version_identifier(value: &str) -> Result<(), AppError> {
    identifier(value)
}

fn timestamp(seconds: i64) -> Result<String, AppError> {
    DateTime::from_timestamp(seconds, 0)
        .map(|d| d.to_rfc3339_opts(SecondsFormat::Secs, true))
        .ok_or_else(|| AppError::InvalidInput("Invalid capture clock".into()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clock {
    pub wall: i64,
    pub monotonic: i64,
    pub boot: String,
}
impl Clock {
    #[cfg(unix)]
    pub fn now() -> Result<Self, AppError> {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // CLOCK_MONOTONIC is system-wide across hook processes, unlike Instant.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
            return Err(AppError::Config(
                "Monotonic clock unavailable; capture paused".into(),
            ));
        }
        #[cfg(target_os = "macos")]
        let boot = Command::new("/usr/sbin/sysctl")
            .args(["-n", "kern.boottime"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| hash(o.stdout));
        #[cfg(not(target_os = "macos"))]
        let boot = fs::read("/proc/sys/kernel/random/boot_id").ok().map(hash);
        Ok(Self {
            wall: Utc::now().timestamp(),
            monotonic: time.tv_sec,
            boot: boot.ok_or_else(|| {
                AppError::Config("Boot identity unavailable; capture paused".into())
            })?,
        })
    }
    #[cfg(not(unix))]
    pub fn now() -> Result<Self, AppError> {
        Err(AppError::Config(
            "Capture timing is not qualified on this platform".into(),
        ))
    }
    fn elapsed(&self, end: &Self) -> Option<i64> {
        let wall = end.wall.checked_sub(self.wall)?;
        let mono = end.monotonic.checked_sub(self.monotonic)?;
        (self.boot == end.boot
            && (0..=MAX_INTERVAL_SECONDS).contains(&wall)
            && (0..=MAX_INTERVAL_SECONDS).contains(&mono)
            && (wall - mono).abs() <= 2)
            .then_some(wall)
    }
}

#[derive(Clone, Debug)]
pub struct Repository {
    pub key: String,
    pub hint_hash: String,
    pub disabled: bool,
}
fn git(directory: &Path, args: &[&str]) -> Result<String, AppError> {
    let result = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .map_err(|_| AppError::Config("Cannot inspect repository identity".into()))?;
    if !result.status.success() {
        return Err(AppError::Config(
            "Capture requires an identifiable Git repository".into(),
        ));
    }
    String::from_utf8(result.stdout)
        .map(|v| v.trim().to_owned())
        .map_err(|_| AppError::Config("Cannot inspect repository identity".into()))
}
impl Repository {
    pub fn resolve(directory: &Path) -> Result<Self, AppError> {
        let root = PathBuf::from(git(directory, &["rev-parse", "--show-toplevel"])?)
            .canonicalize()
            .map_err(storage_error)?;
        let common = git(
            directory,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let common = PathBuf::from(common)
            .canonicalize()
            .map_err(storage_error)?;
        // All remotes participate in consent; adding/replacing one invalidates it.
        // Raw remotes may contain credentials. Only their digest is ever retained.
        let remotes = git(
            directory,
            &["config", "--get-regexp", "^remote\\..*\\.url$"],
        )
        .unwrap_or_default();
        let mut remotes: Vec<_> = remotes.lines().collect();
        remotes.sort_unstable();
        let key = hash(format!(
            "{}\0{}",
            common.to_string_lossy(),
            remotes.join("\n")
        ));
        let hint_path = root.join(".keito/config.yml");
        let mut disabled = false;
        let hint_hash = if hint_path.exists() {
            if fs::symlink_metadata(&hint_path)
                .map_err(storage_error)?
                .file_type()
                .is_symlink()
            {
                return Err(AppError::Config(
                    "Repository mapping hint cannot be a symlink".into(),
                ));
            }
            let file = fs::File::open(&hint_path).map_err(storage_error)?;
            let mut bytes = Vec::new();
            file.take(64 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(storage_error)?;
            if bytes.len() > 64 * 1024 {
                return Err(AppError::Config("Repository mapping hint too large".into()));
            }
            // Hash only mapping/policy fields, never persist the original YAML.
            let yaml: serde_yaml::Value = serde_yaml::from_slice(&bytes)
                .map_err(|_| AppError::Config("Invalid repository mapping hint".into()))?;
            disabled = yaml
                .get("agent_tracking")
                .and_then(|v| v.get("enabled"))
                .and_then(|v| v.as_bool())
                == Some(false);
            let fields = [
                "workspace_id",
                "client_id",
                "project_id",
                "task_id",
                "agent_tracking",
            ];
            let allowlisted: Vec<_> = fields.iter().map(|k| (k, yaml.get(*k).cloned())).collect();
            hash(serde_json::to_vec(&allowlisted).map_err(storage_error)?)
        } else {
            hash(b"no-mapping-hint")
        };
        Ok(Self {
            key,
            hint_hash,
            disabled,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub repository_key: String,
    pub hint_hash: String,
    pub id: String,
    pub version: i64,
    pub workspace_id: String,
    pub owner_id: String,
    pub origin_hash: String,
    pub project_id: String,
    pub task_id: String,
    pub agents: Vec<String>,
    pub paused: bool,
    pub revoked: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub segment_key: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_seconds: i64,
    pub completeness: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ActiveTurn {
    key: String,
    clock: Option<Clock>,
    part: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub source_session_id: String,
    pub source_namespace: String,
    pub harness_version: String,
    pub binding: Binding,
    pub started_at: i64,
    pub observed_through: i64,
    active: Option<ActiveTurn>,
    pub segments: Vec<Segment>,
    queued_segments: usize,
    generation: i64,
    pub revision: i64,
    pub lifecycle: String,
    last_turn: Option<String>,
    seen_turns: std::collections::BTreeSet<String>,
    queued_lifecycle: String,
    pub needs_attention: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturePayload {
    pub schema_version: u8,
    pub source_namespace: String,
    pub source_session_id: String,
    pub revision: i64,
    pub expected_revision: i64,
    pub harness_version: String,
    pub repository_binding_id: String,
    pub binding_version: i64,
    pub project_id: String,
    pub task_id: String,
    pub work_kind: String,
    pub lifecycle: String,
    pub started_at: String,
    pub observed_through: String,
    pub measurement_method: String,
    pub segments: Vec<Segment>,
    pub usage: serde_json::Value,
}
impl Session {
    fn begin(
        binding: &Binding,
        source: &str,
        session_id: &str,
        version: &str,
        clock: &Clock,
    ) -> Self {
        Self {
            id: hash(format!("{}\0{}\0{}", binding.id, source, session_id)),
            source_session_id: session_id.into(),
            source_namespace: source.into(),
            binding: binding.clone(),
            harness_version: version.into(),
            started_at: clock.wall,
            observed_through: clock.wall,
            active: None,
            segments: vec![],
            queued_segments: 0,
            generation: 0,
            revision: 0,
            lifecycle: "ACTIVE".into(),
            last_turn: None,
            seen_turns: Default::default(),
            queued_lifecycle: "ACTIVE".into(),
            needs_attention: false,
        }
    }
    fn close(&mut self, clock: &Clock, complete: bool) -> Result<(), AppError> {
        if let Some(mut active) = self.active.take() {
            if let Some(start) = active.clock.take() {
                let elapsed = start.elapsed(clock).filter(|_| complete);
                let seconds = elapsed.unwrap_or(0);
                let end = if elapsed.is_some() {
                    clock.wall
                } else {
                    start.wall
                };
                self.segments.push(Segment {
                    segment_key: format!("{}-{}", active.key, active.part),
                    started_at: timestamp(start.wall)?,
                    ended_at: timestamp(end)?,
                    duration_seconds: seconds,
                    completeness: if elapsed.is_some() {
                        "COMPLETE"
                    } else {
                        self.needs_attention = true;
                        "INCOMPLETE"
                    }
                    .into(),
                });
            }
            self.seen_turns.insert(active.key.clone());
            self.last_turn = Some(active.key);
        }
        self.observed_through = self.observed_through.max(clock.wall);
        Ok(())
    }
    fn payload(&self, segments: Vec<Segment>, revision: i64) -> Result<CapturePayload, AppError> {
        Ok(CapturePayload {
            schema_version: 1,
            source_namespace: self.source_namespace.clone(),
            source_session_id: self.source_session_id.clone(),
            revision,
            expected_revision: revision - 1,
            harness_version: self.harness_version.clone(),
            repository_binding_id: self.binding.id.clone(),
            binding_version: self.binding.version,
            project_id: self.binding.project_id.clone(),
            task_id: self.binding.task_id.clone(),
            work_kind: "HUMAN_ASSISTED_AGENT_ACTIVITY".into(),
            lifecycle: self.lifecycle.clone(),
            started_at: timestamp(self.started_at)?,
            observed_through: timestamp(self.observed_through)?,
            measurement_method: "OBSERVED_TURN_INTERVALS_V1".into(),
            segments,
            usage: serde_json::json!({"status":"UNAVAILABLE","reason":"NO_QUALIFIED_USAGE_EVIDENCE","models":[]}),
        })
    }
}

pub struct Signal<'a> {
    pub source: &'a str,
    pub source_id: &'a str,
    pub turn_id: Option<&'a str>,
    pub version: &'a str,
    pub event: CaptureEvent,
    pub clock: &'a Clock,
}

pub struct Store {
    connection: Connection,
}
#[derive(Debug, Serialize)]
pub struct Status {
    pub policy: &'static str,
    pub pending: i64,
    pub held: i64,
    pub needs_attention: i64,
    pub delivery_verified: bool,
    pub trusted: &'static str,
    pub bindings: Vec<Binding>,
}
#[derive(Clone, Debug)]
pub struct Queued {
    pub id: i64,
    pub body: String,
    pub hash: String,
    pub attempts: i64,
}
impl Store {
    pub fn open(directory: &Path) -> Result<Self, AppError> {
        if directory.exists()
            && fs::symlink_metadata(directory)
                .map_err(storage_error)?
                .file_type()
                .is_symlink()
        {
            return Err(AppError::Config(
                "Capture storage cannot be a symlink".into(),
            ));
        }
        fs::create_dir_all(directory).map_err(storage_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(storage_error)?;
        }
        let path = directory.join("capture.sqlite3");
        if path.exists()
            && fs::symlink_metadata(&path)
                .map_err(storage_error)?
                .file_type()
                .is_symlink()
        {
            return Err(AppError::Config(
                "Capture database cannot be a symlink".into(),
            ));
        }
        if !path.exists() {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(storage_error(e)),
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(storage_error)?;
        }
        let connection = Connection::open(path).map_err(storage_error)?;
        connection
            .busy_timeout(Duration::from_millis(500))
            .map_err(storage_error)?;
        connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA max_page_count=25600;
            CREATE TABLE IF NOT EXISTS bindings (repo_key TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, binding_id TEXT NOT NULL, source_id TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS outbox (id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, revision INTEGER NOT NULL, body TEXT NOT NULL,
                payload_hash TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0,
                retry_at INTEGER NOT NULL DEFAULT 0, claimed_until INTEGER NOT NULL DEFAULT 0, reason TEXT,
                acknowledged_at INTEGER, UNIQUE(session_id,revision));").map_err(storage_error)?;
        Ok(Self { connection })
    }
    fn transaction(&mut self) -> Result<Transaction<'_>, AppError> {
        self.connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)
    }
    fn check_size(tx: &Transaction<'_>) -> Result<(), AppError> {
        let pages: i64 = tx
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .map_err(storage_error)?;
        let page_size: i64 = tx
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .map_err(storage_error)?;
        if pages * page_size >= MAX_QUEUE_BYTES - 64 * 1024 {
            return Err(AppError::Config(
                "Capture queue limit reached; sync or inspect held evidence. Nothing was evicted."
                    .into(),
            ));
        }
        Ok(())
    }
    pub fn approve(&mut self, mut binding: Binding) -> Result<Binding, AppError> {
        for value in [
            &binding.workspace_id,
            &binding.owner_id,
            &binding.project_id,
            &binding.task_id,
        ] {
            identifier(value)?;
        }
        let tx = self.transaction()?;
        Self::check_size(&tx)?;
        let previous = Self::binding_in(&tx, &binding.repository_key)?;
        if let Some(old) = &previous {
            if !old.revoked
                && old.hint_hash == binding.hint_hash
                && old.workspace_id == binding.workspace_id
                && old.owner_id == binding.owner_id
                && old.origin_hash == binding.origin_hash
                && old.project_id == binding.project_id
                && old.task_id == binding.task_id
                && old.agents.iter().collect::<std::collections::BTreeSet<_>>()
                    == binding
                        .agents
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
            {
                return Ok(old.clone());
            }
        }
        binding.version = previous.as_ref().map_or(1, |b| b.version + 1);
        binding.id = hash(format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{}",
            binding.repository_key,
            binding.workspace_id,
            binding.owner_id,
            binding.project_id,
            binding.task_id,
            binding.version,
            POLICY
        ));
        binding.paused = false;
        binding.revoked = false;
        tx.execute("INSERT INTO bindings(repo_key,data) VALUES(?1,?2) ON CONFLICT(repo_key) DO UPDATE SET data=excluded.data",
            params![binding.repository_key, serde_json::to_string(&binding).map_err(storage_error)?]).map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(binding)
    }
    fn binding_in(connection: &Connection, key: &str) -> Result<Option<Binding>, AppError> {
        let value: Option<String> = connection
            .query_row("SELECT data FROM bindings WHERE repo_key=?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(storage_error)?;
        value
            .map(|v| serde_json::from_str(&v).map_err(storage_error))
            .transpose()
    }
    pub fn binding(&self, key: &str) -> Result<Option<Binding>, AppError> {
        Self::binding_in(&self.connection, key)
    }
    pub fn event(&mut self, repository: &Repository, signal: Signal<'_>) -> Result<(), AppError> {
        let Signal {
            source,
            source_id,
            turn_id,
            version,
            event,
            clock,
        } = signal;
        identifier(source_id)?;
        version_identifier(version)?;
        if matches!(event, CaptureEvent::Prompt | CaptureEvent::Stop) && turn_id.is_none() {
            return Err(AppError::InvalidInput(
                "Missing native turn identity; capture requires qualified prompt_id/turn_id".into(),
            ));
        }
        if let Some(id) = turn_id {
            identifier(id)?;
        }
        let tx = self.transaction()?;
        Self::check_size(&tx)?;
        let Some(binding) = Self::binding_in(&tx, &repository.key)? else {
            return Ok(());
        };
        if repository.disabled
            || binding.revoked
            || binding.hint_hash != repository.hint_hash
            || !binding.agents.iter().any(|a| a == source)
        {
            return Ok(());
        }
        if binding.paused {
            return Ok(());
        }
        let id = hash(format!("{}\0{}\0{}", binding.id, source, source_id));
        let raw: Option<String> = tx
            .query_row("SELECT data FROM sessions WHERE id=?1", [&id], |r| r.get(0))
            .optional()
            .map_err(storage_error)?;
        if raw.is_none() && !matches!(event, CaptureEvent::SessionStart | CaptureEvent::Prompt) {
            return Ok(());
        }
        let mut session: Session = raw
            .map(|v| serde_json::from_str(&v).map_err(storage_error))
            .transpose()?
            .unwrap_or_else(|| Session::begin(&binding, source, source_id, version, clock));
        match event {
            CaptureEvent::SessionStart => {} // Resume must never reset the baseline.
            CaptureEvent::Prompt => {
                if turn_id.is_some_and(|id| session.seen_turns.contains(id)) {
                    return Ok(());
                }
                if let Some(active) = &session.active {
                    if turn_id.is_some_and(|key| key != active.key) {
                        session.close(clock, false)?;
                    } else {
                        return Ok(());
                    } // Concurrent duplicate hook registrations.
                }
                if turn_id.is_some_and(|key| session.last_turn.as_deref() == Some(key)) {
                    return Ok(());
                }
                session.generation += 1;
                let key = turn_id
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("turn-{}", session.generation));
                session.active = Some(ActiveTurn {
                    key,
                    clock: Some(clock.clone()),
                    part: 0,
                });
                session.lifecycle = "ACTIVE".into();
            }
            CaptureEvent::Stop => {
                if session
                    .active
                    .as_ref()
                    .is_some_and(|a| turn_id.is_some_and(|id| id != a.key))
                {
                    return Ok(());
                }
                session.close(clock, true)?;
            }
            CaptureEvent::Interrupt | CaptureEvent::SessionEnd => {
                if event == CaptureEvent::Interrupt
                    && session
                        .active
                        .as_ref()
                        .is_some_and(|a| turn_id.is_some_and(|id| id != a.key))
                {
                    return Ok(());
                }
                session.close(clock, false)?;
                if event == CaptureEvent::SessionEnd {
                    session.lifecycle = if session.needs_attention {
                        "INCOMPLETE"
                    } else {
                        "COMPLETED"
                    }
                    .into();
                }
            }
        }
        Self::queue(&tx, &mut session)?;
        tx.execute("INSERT INTO sessions(id,binding_id,source_id,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            params![id,binding.id,source_id,serde_json::to_string(&session).map_err(storage_error)?]).map_err(storage_error)?;
        tx.commit().map_err(storage_error)
    }
    fn queue(tx: &Transaction<'_>, session: &mut Session) -> Result<(), AppError> {
        if session.active.is_some() {
            return Ok(());
        }
        let final_checkpoint =
            session.revision > 0 && session.queued_lifecycle != session.lifecycle;
        let mut final_checkpoint_sent = false;
        while session.queued_segments < session.segments.len()
            || (final_checkpoint && !final_checkpoint_sent)
        {
            let start = session.queued_segments;
            let mut end = (start + 100).min(session.segments.len());
            let mut body;
            loop {
                let payload =
                    session.payload(session.segments[start..end].to_vec(), session.revision + 1)?;
                body = serde_json::to_string(&payload).map_err(storage_error)?;
                if body.len() <= MAX_BODY_BYTES {
                    break;
                }
                end -= 1;
                if end == start {
                    return Err(AppError::Config(
                        "Capture request exceeds limit; evidence held".into(),
                    ));
                }
            }
            session.revision += 1;
            tx.execute(
                "INSERT INTO outbox(session_id,revision,body,payload_hash) VALUES(?1,?2,?3,?4)",
                params![session.id, session.revision, body, hash(body.as_bytes())],
            )
            .map_err(storage_error)?;
            session.queued_segments = end;
            session.queued_lifecycle = session.lifecycle.clone();
            final_checkpoint_sent = true;
        }
        Ok(())
    }
    pub fn control(
        &mut self,
        repository: &Repository,
        action: &str,
        clock: &Clock,
    ) -> Result<(), AppError> {
        let tx = self.transaction()?;
        let mut binding = Self::binding_in(&tx, &repository.key)?
            .ok_or_else(|| AppError::Config("Repository not approved; run capture setup".into()))?;
        if action == "resume" && (binding.revoked || binding.hint_hash != repository.hint_hash) {
            return Err(AppError::Config(
                "Binding changed or revoked; run capture setup to approve again".into(),
            ));
        }
        let mut statement = tx
            .prepare("SELECT data FROM sessions WHERE binding_id=?1")
            .map_err(storage_error)?;
        let sessions: Vec<String> = statement
            .query_map([&binding.id], |r| r.get(0))
            .map_err(storage_error)?
            .collect::<Result<_, _>>()
            .map_err(storage_error)?;
        drop(statement);
        for raw in sessions {
            let mut session: Session = serde_json::from_str(&raw).map_err(storage_error)?;
            if let Some(mut active) = session.active.take() {
                if action == "pause" {
                    let key = active.key.clone();
                    let part = active.part;
                    session.active = Some(active);
                    session.close(clock, true)?;
                    active = ActiveTurn {
                        key,
                        clock: None,
                        part: part + 1,
                    };
                    session.active = Some(active);
                } else if action == "resume" {
                    active.clock = Some(clock.clone());
                    session.active = Some(active);
                } else {
                    session.active = Some(active);
                    session.close(clock, false)?;
                }
            }
            tx.execute(
                "UPDATE sessions SET data=?2 WHERE id=?1",
                params![
                    session.id,
                    serde_json::to_string(&session).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        }
        binding.paused = action != "resume";
        if action == "disable" {
            binding.revoked = true;
        }
        tx.execute(
            "UPDATE bindings SET data=?2 WHERE repo_key=?1",
            params![
                repository.key,
                serde_json::to_string(&binding).map_err(storage_error)?
            ],
        )
        .map_err(storage_error)?;
        tx.commit().map_err(storage_error)
    }
    pub fn disable_all(&mut self) -> Result<(), AppError> {
        let bindings = self.status()?.bindings;
        let tx = self.transaction()?;
        for mut binding in bindings {
            binding.paused = true;
            binding.revoked = true;
            tx.execute(
                "UPDATE bindings SET data=?2 WHERE repo_key=?1",
                params![
                    binding.repository_key,
                    serde_json::to_string(&binding).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        }
        tx.commit().map_err(storage_error)
    }
    pub fn status(&self) -> Result<Status, AppError> {
        let mut statement = self
            .connection
            .prepare("SELECT data FROM bindings ORDER BY repo_key")
            .map_err(storage_error)?;
        let bindings: Vec<Binding> = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(storage_error)?
            .map(|r| serde_json::from_str(&r.map_err(storage_error)?).map_err(storage_error))
            .collect::<Result<_, _>>()?;
        let pending = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM outbox WHERE state='pending'",
                [],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        let held = self
            .connection
            .query_row("SELECT COUNT(*) FROM outbox WHERE state='held'", [], |r| {
                r.get(0)
            })
            .map_err(storage_error)?;
        let needs_attention = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE json_extract(data,'$.needs_attention')=1",
                [],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        let delivered: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM outbox WHERE state='acknowledged'",
                [],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        Ok(Status {
            policy: POLICY,
            pending,
            held,
            needs_attention,
            delivery_verified: delivered > 0,
            trusted: "UNKNOWN",
            bindings,
        })
    }
    pub fn preview(&self, source_id: &str) -> Result<Vec<Session>, AppError> {
        identifier(source_id)?;
        let mut statement = self
            .connection
            .prepare("SELECT data FROM sessions WHERE source_id=?1 ORDER BY id")
            .map_err(storage_error)?;
        let result = statement
            .query_map([source_id], |r| r.get::<_, String>(0))
            .map_err(storage_error)?
            .map(|r| serde_json::from_str(&r.map_err(storage_error)?).map_err(storage_error))
            .collect();
        result
    }
    /// Claims only the next revision of each session. Lease expiry permits lost-response replay.
    pub fn claim(&mut self, binding: &Binding, now: i64) -> Result<Option<Queued>, AppError> {
        let tx = self.transaction()?;
        let current = Self::binding_in(&tx, &binding.repository_key)?;
        if !current.is_some_and(|b| b.id == binding.id && !b.paused && !b.revoked) {
            return Ok(None);
        }
        let row = tx.query_row("SELECT o.id,o.session_id,o.body,o.payload_hash,o.attempts FROM outbox o JOIN sessions s ON s.id=o.session_id
            WHERE s.binding_id=?1 AND o.state='pending' AND o.retry_at<=?2 AND o.claimed_until<=?2
            AND NOT EXISTS (SELECT 1 FROM outbox prior WHERE prior.session_id=o.session_id AND prior.revision<o.revision AND prior.state!='acknowledged') ORDER BY o.id LIMIT 1",
            params![binding.id,now],|r| Ok(Queued{id:r.get(0)?,body:r.get(2)?,hash:r.get(3)?,attempts:r.get(4)?})).optional().map_err(storage_error)?;
        if let Some(row) = &row {
            tx.execute(
                "UPDATE outbox SET claimed_until=?2 WHERE id=?1",
                params![row.id, now + 30],
            )
            .map_err(storage_error)?;
        }
        tx.commit().map_err(storage_error)?;
        Ok(row)
    }
    pub fn finish(
        &mut self,
        row: &Queued,
        state: &str,
        reason: &str,
        now: i64,
        retry_after: Option<i64>,
    ) -> Result<(), AppError> {
        let delay = retry_after
            .unwrap_or_else(|| (2_i64.pow((row.attempts as u32).min(8)) + (row.id % 7)).min(300))
            .clamp(1, 3600);
        self.connection.execute("UPDATE outbox SET state=?2,reason=?3,attempts=attempts+1,retry_at=?4,claimed_until=0,acknowledged_at=?5 WHERE id=?1 AND payload_hash=?6",
            params![row.id,state,reason,now+delay,if state=="acknowledged" {Some(now)} else {None},row.hash]).map_err(storage_error)?;
        // Only acknowledged payloads expire. Never silently evict unsent evidence.
        self.connection
            .execute(
                "DELETE FROM outbox WHERE state='acknowledged' AND acknowledged_at<?1",
                [now - 7 * 24 * 3600],
            )
            .map_err(storage_error)?;
        Ok(())
    }
}
