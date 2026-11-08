//! Native, recoverable management of only integration-owned adapter registrations.
use super::{InstallOptions, InstallOutcome, UninstallOutcome};
use crate::adapter_files::{checkpoint, digest, failure, hex, identifier, nonce, Dir};
use crate::capture_runtime_pair::CaptureRuntimeIdentity;
use crate::cli::skill::SkillAgent;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const RECEIPT: &str = "capture-install.json";
const JOURNAL: &str = ".keito-capture-publication.json";
const PLUGIN: &str = "keito-time-track@keito";
type Payload = BTreeMap<String, Vec<u8>>;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    adapter_contract: u32,
    agent: String,
    harness_version: String,
    trusted: String,
    delivery_verified: bool,
    generation: String,
    files: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guard_interpreter: Option<Value>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    schema_version: u8,
    stage: Option<String>,
    backup: String,
}
fn agent_name(agent: SkillAgent) -> &'static str {
    match agent {
        SkillAgent::Codex => "codex",
        SkillAgent::ClaudeCode => "claude-code",
    }
}
fn host_name(agent: SkillAgent) -> &'static str {
    match agent {
        SkillAgent::Codex => ".codex",
        SkillAgent::ClaudeCode => ".claude",
    }
}
fn config_name(agent: SkillAgent) -> &'static str {
    match agent {
        SkillAgent::Codex => "hooks.json",
        SkillAgent::ClaudeCode => "settings.json",
    }
}
fn payload_tree(dir: &Dir, prefix: &str, out: &mut Payload, depth: usize) -> Result<(), AppError> {
    if depth > 16 {
        return Err(failure());
    }
    for entry in dir.entries()? {
        if entry == RECEIPT && prefix.is_empty() {
            continue;
        }
        let relative = if prefix.is_empty() {
            entry.clone()
        } else {
            format!("{prefix}/{entry}")
        };
        if let Ok(child) = dir.child(&entry, false) {
            payload_tree(&child, &relative, out, depth + 1)?;
        } else {
            let bytes = dir.read(&entry, 8 * 1024 * 1024)?.ok_or_else(failure)?;
            if out.len() >= 4096
                || out.values().map(Vec::len).sum::<usize>() + bytes.len() > 32 * 1024 * 1024
            {
                return Err(failure());
            }
            out.insert(relative, bytes);
        }
    }
    Ok(())
}
fn tree(dir: &Dir) -> Result<Payload, AppError> {
    let mut out = BTreeMap::new();
    payload_tree(dir, "", &mut out, 0)?;
    Ok(out)
}
fn hashes(payload: &Payload) -> BTreeMap<String, String> {
    payload
        .iter()
        .map(|(k, v)| (k.clone(), digest(v)))
        .collect()
}
fn generation_key(
    agent: &str,
    version: &str,
    files: &BTreeMap<String, String>,
    runtime: &Value,
) -> Result<String, AppError> {
    Ok(digest(
        serde_json::to_vec(&serde_json::json!([agent, version, files, runtime]))
            .map_err(|_| failure())?,
    ))
}
fn valid_runtime(value: &Value) -> bool {
    let Some(v) = value.as_object() else {
        return false;
    };
    if v.len() != 7 {
        return false;
    }
    ["core_path", "guard_path"].iter().all(|k| {
        v.get(*k).and_then(Value::as_str).is_some_and(|s| {
            Path::new(s).is_absolute()
                && !Path::new(s).components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                })
        })
    }) && ["core_sha256", "guard_sha256"].iter().all(|k| {
        v.get(*k)
            .and_then(Value::as_str)
            .is_some_and(|s| hex(s, 64))
    }) && ["version", "target"].iter().all(|k| {
        v.get(*k)
            .and_then(Value::as_str)
            .is_some_and(|s| identifier(s, 128))
    }) && v.get("guard_contract").and_then(Value::as_u64) == Some(1)
}
fn validate_receipt(dir: &Dir) -> Result<Receipt, AppError> {
    let receipt: Receipt =
        serde_json::from_slice(&dir.read(RECEIPT, 512 * 1024)?.ok_or_else(failure)?)
            .map_err(|_| failure())?;
    if !matches!(receipt.adapter_contract, 1 | 2)
        || !matches!(receipt.agent.as_str(), "codex" | "claude-code")
        || !identifier(&receipt.harness_version, 64)
        || !hex(&receipt.generation, 64)
        || receipt.files.is_empty()
    {
        return Err(failure());
    }
    for (path, hash) in &receipt.files {
        if Path::new(path).is_absolute()
            || path
                .split('/')
                .any(|c| c.is_empty() || c == "." || c == "..")
            || !hex(hash, 64)
        {
            return Err(failure());
        }
    }
    if receipt.adapter_contract == 2 {
        let runtime = receipt
            .runtime
            .as_ref()
            .filter(|v| valid_runtime(v))
            .ok_or_else(failure)?;
        if receipt.guard_interpreter.is_some()
            || generation_key(
                &receipt.agent,
                &receipt.harness_version,
                &receipt.files,
                runtime,
            )? != receipt.generation
        {
            return Err(failure());
        }
    } else if let Some(pin) = &receipt.guard_interpreter {
        let p = pin
            .as_object()
            .filter(|p| p.len() == 2)
            .ok_or_else(failure)?;
        if !p
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|s| Path::new(s).is_absolute())
            || !p
                .get("sha256")
                .and_then(Value::as_str)
                .is_some_and(|s| hex(s, 64))
            || generation_key(
                &receipt.agent,
                &receipt.harness_version,
                &receipt.files,
                pin,
            )? != receipt.generation
        {
            return Err(failure());
        }
    }
    let payload = tree(dir)?;
    if hashes(&payload) != receipt.files {
        return Err(failure());
    }
    validate_modes(dir, "")?;
    Ok(receipt)
}
fn validate_expected_receipt(dir: &Dir, expected: &[u8]) -> Result<(), AppError> {
    validate_receipt(dir)?;
    if dir.read(RECEIPT, 512 * 1024)?.as_deref() != Some(expected) {
        return Err(failure());
    }
    Ok(())
}
fn validate_modes(dir: &Dir, prefix: &str) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    if dir.metadata()?.permissions().mode() & 0o500 != 0o500 {
        return Err(failure());
    }
    for entry in dir.entries()? {
        if let Ok(child) = dir.child(&entry, false) {
            validate_modes(&child, &format!("{prefix}{entry}/"))?;
        } else {
            let mode = dir
                .open(&entry)?
                .metadata()
                .map_err(|_| failure())?
                .permissions()
                .mode();
            if mode & 0o400 == 0 || (entry.ends_with(".sh") && mode & 0o100 == 0) {
                return Err(failure());
            }
        }
    }
    Ok(())
}
fn write_payload(dir: &Dir, payload: &Payload) -> Result<(), AppError> {
    for (relative, bytes) in payload {
        let mut target = dir.clone();
        let mut parts = relative.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                target.atomic(
                    part,
                    bytes,
                    if part.ends_with(".sh") { 0o700 } else { 0o600 },
                    Some(None),
                    "install-payload-file-fsynced",
                )?;
            } else {
                target = target.child(part, true)?;
            }
        }
    }
    dir.sync()
}
fn parse_config(bytes: Option<&[u8]>) -> Result<Value, AppError> {
    let value = match bytes {
        Some(b) => serde_json::from_slice::<Value>(b).map_err(|_| failure())?,
        None => serde_json::json!({}),
    };
    let obj = value.as_object().ok_or_else(failure)?;
    if let Some(hooks) = obj.get("hooks") {
        for groups in hooks.as_object().ok_or_else(failure)?.values() {
            for group in groups.as_array().ok_or_else(failure)? {
                if group.as_object().is_none()
                    || group
                        .get("hooks")
                        .and_then(Value::as_array)
                        .ok_or_else(failure)?
                        .iter()
                        .any(|h| !h.is_object())
                {
                    return Err(failure());
                }
            }
        }
    }
    if obj.get("enabledPlugins").is_some_and(|v| !v.is_object()) {
        return Err(failure());
    }
    Ok(value)
}
/// Literal POSIX words only. Expansion, executable combinations and uncertainty
/// preserve the original command. Double-quoted backslash semantics are POSIX.
fn words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut active = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' || c == '\r' {
            return None;
        }
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else {
                    word.push(c);
                }
            }
            _ => {
                if c == '\\' {
                    let next = chars.next()?;
                    if next == '\n' || next == '\r' {
                        return None;
                    }
                    if quote == Some('"') && !"$`\"\\".contains(next) {
                        word.push('\\');
                    }
                    word.push(next);
                    active = true;
                    continue;
                }
                if c == '\'' && quote.is_none() {
                    quote = Some('\'');
                    active = true;
                    continue;
                }
                if c == '"' {
                    quote = if quote == Some('"') { None } else { Some('"') };
                    active = true;
                    continue;
                }
                if "$`".contains(c) || (quote.is_none() && ";&|<>(){}*?[]~#".contains(c)) {
                    return None;
                }
                if c.is_whitespace() && quote.is_none() {
                    if active {
                        words.push(std::mem::take(&mut word));
                        active = false;
                    }
                    continue;
                }
                word.push(c);
            }
        }
        active = true;
    }
    if quote.is_some() {
        return None;
    }
    if active {
        words.push(word);
    }
    Some(words)
}
fn resolve_existing_prefix(path: &Path) -> Option<PathBuf> {
    let mut prefix = path.to_path_buf();
    let mut tail = Vec::new();
    loop {
        if let Ok(mut canonical) = prefix.canonicalize() {
            for part in tail.iter().rev() {
                canonical.push(part);
            }
            return Some(canonical);
        }
        tail.push(prefix.file_name()?.to_os_string());
        if !prefix.pop() {
            return None;
        }
    }
}
fn owned(hook: &Value, dest: &Path, agent: &str) -> Option<bool> {
    if hook.get("type")?.as_str()? != "command" {
        return None;
    }
    let mut argv = words(hook.get("command")?.as_str()?)?;
    if argv
        .first()
        .is_some_and(|s| matches!(s.as_str(), "env" | "/usr/bin/env"))
    {
        argv.remove(0);
    }
    let mut environment = BTreeMap::new();
    while let Some((key, value)) = argv.first().and_then(|s| s.split_once('=')) {
        if !matches!(
            key,
            "KEITO_CLI_BIN"
                | "KEITO_CAPTURE_GUARD_BIN"
                | "KEITO_CAPTURE_CORE_SHA256"
                | "KEITO_AGENT_TYPE"
                | "KEITO_HARNESS_VERSION"
                | "KEITO_CAPTURE_PYTHON"
        ) || environment
            .insert(key.to_owned(), value.to_owned())
            .is_some()
        {
            return None;
        }
        argv.remove(0);
    }
    if argv
        .first()
        .is_some_and(|s| matches!(s.as_str(), "bash" | "/bin/bash" | "/usr/bin/bash"))
    {
        argv.remove(0);
    }
    let supplied = Path::new(argv.first()?);
    if !supplied.is_absolute()
        || supplied.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return None;
    }
    let resolved = resolve_existing_prefix(supplied)?;
    let executable = resolved.as_path();
    if !executable.is_absolute()
        || executable.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return None;
    }
    let script = executable.file_name()?.to_str()?;
    let scripts = [
        "dispatch.sh",
        "session-start.sh",
        "session-end.sh",
        "session-finalize.sh",
        "prompt.sh",
        "stop.sh",
        "interrupt.sh",
    ];
    let legacy =
        executable.parent() == Some(dest.join("hooks").as_path()) && scripts.contains(&script);
    let generation = executable.parent()?.parent()?;
    let host = dest.parent()?.parent()?;
    let generated = script == "dispatch.sh"
        && executable.parent()?.file_name()?.to_str()? == "hooks"
        && generation.parent() == Some(host.join(".keito-capture-generations").as_path())
        && hex(generation.file_name()?.to_str()?, 64);
    if !legacy && !generated {
        return None;
    }
    let args = &argv[1..];
    if script != "dispatch.sh" {
        return (args.is_empty()
            && environment.get("KEITO_AGENT_TYPE").map(String::as_str) == Some(agent))
        .then_some(true);
    }
    if args.len() != 6 {
        return None;
    }
    let mut flags = BTreeMap::new();
    for pair in args.as_chunks::<2>().0 {
        if !matches!(
            pair[0].as_str(),
            "--agent" | "--event" | "--harness-version"
        ) || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return None;
        }
    }
    (flags.get("--agent") == Some(&agent)
        && flags.get("--event").is_some_and(|e| {
            matches!(
                *e,
                "session-start" | "prompt" | "stop" | "session-end" | "interrupt"
            )
        })
        && flags
            .get("--harness-version")
            .is_some_and(|v| identifier(v, 64)))
    .then_some(false)
}
fn strip(value: &mut Value, dest: &Path, agent: &str) -> (usize, bool) {
    let mut removed = 0;
    let mut legacy = false;
    if let Some(events) = value.get_mut("hooks").and_then(Value::as_object_mut) {
        events.retain(|_, groups| {
            groups.as_array_mut().unwrap().retain_mut(|group| {
                let hooks = group["hooks"].as_array_mut().unwrap();
                let empty = hooks.is_empty();
                hooks.retain(|h| {
                    if let Some(old) = owned(h, dest, agent) {
                        removed += 1;
                        legacy |= old;
                        false
                    } else {
                        true
                    }
                });
                empty || !hooks.is_empty()
            });
            !groups.as_array().unwrap().is_empty()
        });
    }
    (removed, legacy)
}
fn recovery_name(value: &str, prefix: &str, length: Option<usize>) -> bool {
    value.strip_prefix(prefix).is_some_and(|s| {
        !s.is_empty()
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            && length.is_none_or(|n| hex(s, n))
    })
}
fn recover(host: &Dir, skills: &Dir) -> Result<(), AppError> {
    let Some(bytes) = host.read(JOURNAL, 512 * 1024)? else {
        return Ok(());
    };
    let journal: Publication = serde_json::from_slice(&bytes).map_err(|_| failure())?;
    if journal.schema_version != 1
        || !recovery_name(&journal.backup, ".keito-time-track-backup-", Some(32))
        || journal
            .stage
            .as_ref()
            .is_some_and(|s| !recovery_name(s, ".keito-time-track-stage-", None))
    {
        return Err(failure());
    }
    if skills.exists(&journal.backup)? {
        skills.child(&journal.backup, false)?;
        if !skills.exists("keito-time-track")? {
            skills.rename(&journal.backup, skills, "keito-time-track")?;
        }
    }
    if let Some(stage) = journal.stage {
        if skills.exists(&stage)? {
            let staged = skills.child(&stage, false)?;
            validate_receipt(&staged)?;
            skills.remove_tree(&stage)?;
        }
    }
    host.unlink(JOURNAL, false)
}
fn runtime_unchanged(runtime: &CaptureRuntimeIdentity) -> Result<(), AppError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let value = serde_json::to_value(runtime).map_err(|_| failure())?;
    if !valid_runtime(&value) {
        return Err(failure());
    }
    for (path, expected) in [
        (&runtime.core_path, &runtime.core_sha256),
        (&runtime.guard_path, &runtime.guard_sha256),
    ] {
        let parent = Dir::anchor(path.parent().ok_or_else(failure)?)?;
        let file = parent.open(
            path.file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(failure)?,
        )?;
        let info = file.metadata().map_err(|_| failure())?;
        if info.len() > 512 * 1024 * 1024 || info.permissions().mode() & 0o100 == 0 {
            return Err(failure());
        }
        let mut hasher = Sha256::new();
        let mut file = file.take(512 * 1024 * 1024 + 1);
        let mut block = [0u8; 64 * 1024];
        loop {
            let n = file.read(&mut block).map_err(|_| failure())?;
            if n == 0 {
                break;
            }
            hasher.update(&block[..n]);
        }
        let hash: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if &hash != expected {
            return Err(failure());
        }
    }
    Ok(())
}
fn detect_version(agent: SkillAgent) -> String {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let binary = if agent == SkillAgent::Codex {
        "codex"
    } else {
        "claude"
    };
    let Ok(mut child) = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return "unknown".into();
    };
    let Some(mut pipe) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return "unknown".into();
    };
    unsafe {
        let flags = libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let started = Instant::now();
    let mut bytes = Vec::new();
    let status = loop {
        let mut block = [0u8; 1024];
        match pipe.read(&mut block) {
            Ok(n) => bytes.extend_from_slice(&block[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break None,
        }
        if bytes.len() > 8192 || started.elapsed() > Duration::from_secs(1) {
            break None;
        }
        match child.try_wait() {
            Ok(Some(s)) => {
                let _ = pipe.take(8193 - bytes.len() as u64).read_to_end(&mut bytes);
                break Some(s);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    if !status.is_some_and(|s| s.success()) {
        let _ = child.kill();
        let _ = child.wait();
        return "unknown".into();
    }
    String::from_utf8_lossy(&bytes)
        .split_whitespace()
        .find(|s| {
            identifier(s, 64)
                && s.split('.').take(3).count() == 3
                && s.as_bytes()[0].is_ascii_digit()
        })
        .unwrap_or("unknown")
        .to_owned()
}
pub fn install(options: InstallOptions<'_>) -> Result<InstallOutcome, AppError> {
    let version = options
        .harness_version
        .map(str::to_owned)
        .unwrap_or_else(|| detect_version(options.agent));
    if !identifier(&version, 64) {
        return Err(AppError::InvalidInput("Harness version is incompatible with the capture identifier contract; review and qualify the exact version without truncating it.".into()));
    }
    runtime_unchanged(options.runtime)?;
    let payload = tree(&Dir::anchor(options.source_root)?)?;
    if payload.is_empty()
        || !payload.contains_key("SKILL.md")
        || !payload.contains_key("hooks/dispatch.sh")
        || payload.keys().any(|p| {
            p.ends_with(".py")
                || p.starts_with("tests/")
                || p.split('/').any(|c| c == "__pycache__")
        })
    {
        return Err(failure());
    }
    let home = Dir::anchor(options.profile_home)?;
    let host = home.child(host_name(options.agent), true)?;
    let _lock = host.lock(&format!("{}.keito.lock", config_name(options.agent)))?;
    // Check each parent before publishing a generation or a configuration.
    let skills = host.child("skills", true)?;
    recover(&host, &skills)?;
    let config = config_name(options.agent);
    let original = host.read(config, 1024 * 1024)?;
    let mut data = parse_config(original.as_deref())?;
    let dest = skills.path.join("keito-time-track");
    let (_, legacy) = strip(&mut data, &dest, agent_name(options.agent));
    let plugin = options.agent == SkillAgent::ClaudeCode
        && data
            .get("enabledPlugins")
            .and_then(|p| p.get(PLUGIN))
            .and_then(Value::as_bool)
            == Some(true);
    if (legacy || plugin) && !options.confirm_migration {
        return Err(AppError::Conflict("Legacy Keito hooks/plugin require review; retry with --confirm-migration only after reviewing the migration.".into()));
    }
    let exists = skills.exists("keito-time-track")?;
    let known = exists
        && skills
            .child("keito-time-track", false)
            .and_then(|d| validate_receipt(&d))
            .is_ok();
    let original_receipt = if known {
        skills
            .child("keito-time-track", false)?
            .read(RECEIPT, 512 * 1024)?
    } else {
        None
    };
    if exists && !known && !options.confirm_migration {
        return Err(failure());
    }
    // Even confirmed legacy migration must never move a symlink or non-directory.
    if exists {
        skills.child("keito-time-track", false)?;
    }
    if plugin {
        data["enabledPlugins"][PLUGIN] = Value::Bool(false);
    }
    let runtime = serde_json::to_value(options.runtime).map_err(|_| failure())?;
    let files = hashes(&payload);
    let key = generation_key(agent_name(options.agent), &version, &files, &runtime)?;
    let generations = host.child(".keito-capture-generations", true)?;
    let stage_name = format!(".keito-time-track-stage-{}", &nonce()?[..32]);
    let staged = generations.child(&stage_name, true)?;
    let mut complete = payload.clone();
    let receipt = Receipt {
        adapter_contract: 2,
        agent: agent_name(options.agent).into(),
        harness_version: version.clone(),
        trusted: "unknown".into(),
        delivery_verified: false,
        generation: key.clone(),
        files,
        runtime: Some(runtime),
        guard_interpreter: None,
    };
    complete.insert(
        RECEIPT.into(),
        serde_json::to_vec_pretty(&receipt).map_err(|_| failure())?,
    );
    write_payload(&staged, &complete)?;
    if generations.exists(&key)? {
        let existing = generations.child(&key, false)?;
        let old = validate_receipt(&existing)?;
        if old.adapter_contract != 2
            || old.generation != key
            || old.runtime != receipt.runtime
            || tree(&existing)? != payload
        {
            return Err(failure());
        }
        generations.remove_tree(&stage_name)?;
    } else {
        generations.rename(&stage_name, &generations, &key)?;
        checkpoint("install-generation-published");
    }
    let generation = generations.child(&key, false)?;
    validate_receipt(&generation)?;
    let discovery_stage = format!(".keito-time-track-stage-{}", &nonce()?[..32]);
    let staged = skills.child(&discovery_stage, true)?;
    write_payload(&staged, &complete)?;
    let hooks = data
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .unwrap();
    let events = [
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "prompt"),
        ("Stop", "stop"),
        ("SessionEnd", "session-end"),
        (
            if options.agent == SkillAgent::ClaudeCode {
                "StopFailure"
            } else {
                "Interrupt"
            },
            "interrupt",
        ),
    ];
    for (event, lifecycle) in events {
        let command=format!("KEITO_CLI_BIN={} KEITO_CAPTURE_GUARD_BIN={} KEITO_CAPTURE_CORE_SHA256={} {} --agent {} --event {} --harness-version {}",quote(options.runtime.core_path.to_str().ok_or_else(failure)?),quote(options.runtime.guard_path.to_str().ok_or_else(failure)?),options.runtime.core_sha256,quote(generation.path.join("hooks/dispatch.sh").to_str().ok_or_else(failure)?),agent_name(options.agent),lifecycle,quote(&version));
        hooks
            .entry(event)
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"hooks":[{"type":"command","command":command,"timeout":1}]}));
    }
    let backup = format!(".keito-time-track-backup-{}", &nonce()?[..32]);
    let publication = Publication {
        schema_version: 1,
        stage: Some(discovery_stage.clone()),
        backup: backup.clone(),
    };
    let result = (|| {
        host.atomic(
            JOURNAL,
            &serde_json::to_vec(&publication).map_err(|_| failure())?,
            0o600,
            Some(None),
            "install-journal-fsynced",
        )?;
        runtime_unchanged(options.runtime)?;
        if let Some(receipt) = &original_receipt {
            validate_expected_receipt(&skills.child("keito-time-track", false)?, receipt)?;
        }
        host.atomic(
            config,
            &serde_json::to_vec_pretty(&data).map_err(|_| failure())?,
            0o600,
            Some(original.as_deref()),
            "install-config-fsynced",
        )?;
        checkpoint("install-config-published");
        if exists {
            if let Some(receipt) = &original_receipt {
                validate_expected_receipt(&skills.child("keito-time-track", false)?, receipt)?;
            }
            skills.rename("keito-time-track", &skills, &backup)?;
            checkpoint("install-payload-backup");
            if let Some(receipt) = &original_receipt {
                validate_expected_receipt(&skills.child(&backup, false)?, receipt)?;
            }
        }
        skills.rename(&discovery_stage, &skills, "keito-time-track")?;
        checkpoint("install-discovery-published");
        host.unlink(JOURNAL, false)
    })();
    if host.exists(JOURNAL)? {
        recover(&host, &skills)?;
    } else if skills.exists(&discovery_stage)? {
        skills.remove_tree(&discovery_stage)?;
    }
    result?;
    if let Some(receipt) = &original_receipt {
        if skills.exists(&backup)? {
            validate_expected_receipt(&skills.child(&backup, false)?, receipt)?;
            skills.remove_tree(&backup)?;
        }
    }
    Ok(InstallOutcome {
        generation: key,
        hooks_registered: 5,
        migrated: legacy || plugin || (exists && !known),
    })
}
pub fn uninstall(profile_home: &Path, agent: SkillAgent) -> Result<UninstallOutcome, AppError> {
    let home = Dir::anchor(profile_home)?;
    if !home.exists(host_name(agent))? {
        return Ok(UninstallOutcome {
            hooks_removed: 0,
            payload_removed: false,
            retained_generations: 0,
        });
    }
    let host = home.child(host_name(agent), false)?;
    let _lock = host.lock(&format!("{}.keito.lock", config_name(agent)))?;
    let skills = host.child("skills", true)?;
    recover(&host, &skills)?;
    let config = config_name(agent);
    let original = host.read(config, 1024 * 1024)?;
    let mut data = parse_config(original.as_deref())?;
    let exists = skills.exists("keito-time-track")?;
    if exists {
        validate_receipt(&skills.child("keito-time-track", false)?)?;
    }
    let original_receipt = if exists {
        skills
            .child("keito-time-track", false)?
            .read(RECEIPT, 512 * 1024)?
    } else {
        None
    };
    let (removed, _) = strip(
        &mut data,
        &skills.path.join("keito-time-track"),
        agent_name(agent),
    );
    let backup = format!(".keito-time-track-backup-{}", &nonce()?[..32]);
    let publication = Publication {
        schema_version: 1,
        stage: None,
        backup: backup.clone(),
    };
    let result = (|| {
        host.atomic(
            JOURNAL,
            &serde_json::to_vec(&publication).map_err(|_| failure())?,
            0o600,
            Some(None),
            "uninstall-journal-fsynced",
        )?;
        if let Some(receipt) = &original_receipt {
            validate_expected_receipt(&skills.child("keito-time-track", false)?, receipt)?;
        }
        host.atomic(
            config,
            &serde_json::to_vec_pretty(&data).map_err(|_| failure())?,
            0o600,
            Some(original.as_deref()),
            "uninstall-config-fsynced",
        )?;
        if exists {
            if let Some(receipt) = &original_receipt {
                validate_expected_receipt(&skills.child("keito-time-track", false)?, receipt)?;
            }
            skills.rename("keito-time-track", &skills, &backup)?;
            checkpoint("uninstall-payload-backup");
            if let Some(receipt) = &original_receipt {
                validate_expected_receipt(&skills.child(&backup, false)?, receipt)?;
            }
        }
        host.unlink(JOURNAL, false)
    })();
    if host.exists(JOURNAL)? {
        recover(&host, &skills)?;
    }
    result?;
    if let Some(receipt) = &original_receipt {
        if skills.exists(&backup)? {
            validate_expected_receipt(&skills.child(&backup, false)?, receipt)?;
            skills.remove_tree(&backup)?;
        }
    }
    let retained = if host.exists(".keito-capture-generations")? {
        host.child(".keito-capture-generations", false)?
            .entries()?
            .iter()
            .filter(|s| hex(s, 64))
            .count()
    } else {
        0
    };
    Ok(UninstallOutcome {
        hooks_removed: removed,
        payload_removed: exists,
        retained_generations: retained,
    })
}

/// Read-only configuration inspection. Runtime identity must come from the
/// caller's release-anchor verifier; no binaries, network or version probes run.
/// Invalid, legacy, pending or incomplete local state is conservatively false.
pub fn inspect(
    profile_home: &Path,
    agent: SkillAgent,
    runtime: Option<&CaptureRuntimeIdentity>,
) -> Result<bool, AppError> {
    let Some(runtime) = runtime else {
        return Ok(false);
    };
    let checked = (|| -> Result<bool, AppError> {
        runtime_unchanged(runtime)?;
        let home = Dir::anchor(profile_home)?;
        if !home.exists(host_name(agent))? {
            return Ok(false);
        }
        let host = home.child(host_name(agent), false)?;
        if host.exists(JOURNAL)? {
            return Ok(false);
        }
        let skills = host.child("skills", false)?;
        let dest = skills.child("keito-time-track", false)?;
        let receipt = validate_receipt(&dest)?;
        let serialized_runtime = serde_json::to_value(runtime).map_err(|_| failure())?;
        if receipt.adapter_contract != 2
            || receipt.agent != agent_name(agent)
            || receipt.harness_version == "unknown"
            || receipt.files.keys().any(|p| {
                p.ends_with(".py")
                    || p.ends_with(".pyc")
                    || p.starts_with("tests/")
                    || p.split('/').any(|c| c == "__pycache__")
            })
            || receipt.runtime.as_ref() != Some(&serialized_runtime)
            || receipt.trusted != "unknown"
            || receipt.delivery_verified
        {
            return Ok(false);
        }
        let generation = host
            .child(".keito-capture-generations", false)?
            .child(&receipt.generation, false)?;
        let generation_receipt = validate_receipt(&generation)?;
        if serde_json::to_value(&generation_receipt).map_err(|_| failure())?
            != serde_json::to_value(&receipt).map_err(|_| failure())?
        {
            return Ok(false);
        }
        let original = host.read(config_name(agent), 1024 * 1024)?;
        let data = parse_config(original.as_deref())?;
        if agent == SkillAgent::ClaudeCode
            && data
                .get("enabledPlugins")
                .and_then(|p| p.get(PLUGIN))
                .and_then(Value::as_bool)
                == Some(true)
        {
            return Ok(false);
        }
        let Some(events) = data.get("hooks").and_then(Value::as_object) else {
            return Ok(false);
        };
        let expected_events = [
            ("SessionStart", "session-start"),
            ("UserPromptSubmit", "prompt"),
            ("Stop", "stop"),
            ("SessionEnd", "session-end"),
            (
                if agent == SkillAgent::ClaudeCode {
                    "StopFailure"
                } else {
                    "Interrupt"
                },
                "interrupt",
            ),
        ];
        let mut seen = BTreeMap::new();
        for (event, groups) in events {
            for group in groups.as_array().ok_or_else(failure)? {
                for hook in group["hooks"].as_array().ok_or_else(failure)? {
                    let command = hook.get("command").and_then(Value::as_str).unwrap_or("");
                    let actual = words(command);
                    let expected = expected_events.iter().find(|(name, _)| name == event).map(
                        |(_, lifecycle)| {
                            vec![
                                format!("KEITO_CLI_BIN={}", runtime.core_path.display()),
                                format!("KEITO_CAPTURE_GUARD_BIN={}", runtime.guard_path.display()),
                                format!("KEITO_CAPTURE_CORE_SHA256={}", runtime.core_sha256),
                                generation
                                    .path
                                    .join("hooks/dispatch.sh")
                                    .display()
                                    .to_string(),
                                "--agent".into(),
                                agent_name(agent).into(),
                                "--event".into(),
                                (*lifecycle).into(),
                                "--harness-version".into(),
                                receipt.harness_version.clone(),
                            ]
                        },
                    );
                    if actual
                        .as_ref()
                        .is_some_and(|v| Some(v) == expected.as_ref())
                    {
                        if hook.as_object().is_none_or(|h| h.len() != 3)
                            || hook.get("type").and_then(Value::as_str) != Some("command")
                            || hook.get("timeout").and_then(Value::as_u64) != Some(1)
                            || group.as_object().is_none_or(|g| g.len() != 1)
                            || group["hooks"].as_array().is_none_or(|h| h.len() != 1)
                        {
                            return Ok(false);
                        }
                        *seen.entry(event.as_str()).or_insert(0usize) += 1;
                    } else if owned(hook, &dest.path, agent_name(agent)).is_some()
                        || actual
                            .as_ref()
                            .is_some_and(|v| invokes_adapter(v, &dest.path, &host.path))
                        || (actual.is_none()
                            && (command.contains(".keito-capture-generations/")
                                || command.contains("/skills/keito-time-track/hooks/")))
                    {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(expected_events
            .iter()
            .all(|(name, _)| seen.get(name) == Some(&1))
            && seen.len() == 5)
    })();
    Ok(checked.unwrap_or(false))
}
fn invokes_adapter(words: &[String], dest: &Path, host: &Path) -> bool {
    let mut argv = words;
    if argv
        .first()
        .is_some_and(|s| matches!(s.as_str(), "env" | "/usr/bin/env"))
    {
        argv = &argv[1..];
    }
    while argv.first().is_some_and(|s| {
        s.split_once('=').is_some_and(|(key, _)| {
            !key.is_empty() && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
    }) {
        argv = &argv[1..];
    }
    if argv
        .first()
        .is_some_and(|s| matches!(s.as_str(), "bash" | "/bin/bash" | "/usr/bin/bash"))
    {
        argv = &argv[1..];
    }
    let Some(path) = argv
        .first()
        .and_then(|s| resolve_existing_prefix(Path::new(s)))
    else {
        return false;
    };
    path.starts_with(dest.join("hooks"))
        || path.starts_with(host.join(".keito-capture-generations"))
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::adapter_files::set_hook;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    struct Fixture {
        root: tempfile::TempDir,
        home: PathBuf,
        source: PathBuf,
        runtime: CaptureRuntimeIdentity,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("home with ' literal $");
            let source = root.path().join("source");
            fs::create_dir(&home).unwrap();
            fs::create_dir(home.join("capture")).unwrap();
            fs::create_dir_all(source.join("hooks")).unwrap();
            fs::write(source.join("SKILL.md"), "synthetic native skill").unwrap();
            fs::write(source.join("hooks/dispatch.sh"), "#!/bin/sh\nexit 0\n").unwrap();
            let core = root.path().join("core");
            let guard = root.path().join("guard");
            fs::write(&core, b"synthetic core").unwrap();
            fs::write(&guard, b"synthetic guard").unwrap();
            for p in [&core, &guard] {
                fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let runtime = CaptureRuntimeIdentity {
                core_path: core,
                guard_path: guard,
                core_sha256: digest(b"synthetic core"),
                guard_sha256: digest(b"synthetic guard"),
                version: "0.2.0".into(),
                target: "aarch64-apple-darwin".into(),
                guard_contract: 1,
            };
            Self {
                root,
                home,
                source,
                runtime,
            }
        }
        fn install(
            &self,
            agent: SkillAgent,
            version: &str,
            migrate: bool,
        ) -> Result<InstallOutcome, AppError> {
            install(InstallOptions {
                source_root: &self.source,
                profile_home: &self.home,
                agent,
                runtime: &self.runtime,
                confirm_migration: migrate,
                harness_version: Some(version),
            })
        }
        fn config(&self) -> PathBuf {
            self.home.join(".codex/hooks.json")
        }
        fn json(&self) -> Value {
            serde_json::from_slice(&fs::read(self.config()).unwrap()).unwrap()
        }
        fn seed(&self, value: Value) {
            fs::create_dir_all(self.home.join(".codex")).unwrap();
            fs::write(self.config(), serde_json::to_vec(&value).unwrap()).unwrap();
        }
    }
    #[test]
    fn native_install_is_immutable_idempotent_and_records_runtime_pair() {
        let f = Fixture::new();
        let first = f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        assert_eq!(first.hooks_registered, 5);
        let config = f.json();
        let second = f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        assert_eq!(first.generation, second.generation);
        assert_eq!(config, f.json());
        let receipt: Value = serde_json::from_slice(
            &fs::read(
                f.home
                    .join(".codex/skills/keito-time-track/capture-install.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["adapter_contract"], 2);
        assert_eq!(receipt["runtime"]["guard_sha256"], f.runtime.guard_sha256);
        assert_eq!(receipt["trusted"], "unknown");
        assert_eq!(receipt["delivery_verified"], false);
        for groups in config["hooks"].as_object().unwrap().values() {
            let c = groups[0]["hooks"][0]["command"].as_str().unwrap();
            assert!(c.contains("KEITO_CAPTURE_GUARD_BIN="));
            assert!(c.contains(&format!(
                "KEITO_CAPTURE_CORE_SHA256={}",
                f.runtime.core_sha256
            )));
            assert!(!c.contains("PYTHON"));
            assert_eq!(groups[0]["hooks"][0]["timeout"], 1);
        }
    }
    #[test]
    fn grouped_foreign_commands_plugin_fields_and_empty_groups_survive() {
        let f = Fixture::new();
        let foreign = serde_json::json!({"type":"command","command":"audit --safe"});
        f.seed(serde_json::json!({"hooks":{"Stop":[{"matcher":"anything","timeout":9,"hooks":[foreign.clone()]},{"hooks":[]}]},"enabledPlugins":{"other@vendor":true},"description":"preserve"}));
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let out = uninstall(&f.home, SkillAgent::Codex).unwrap();
        assert_eq!(out.hooks_removed, 5);
        assert!(out.payload_removed);
        assert!(out.retained_generations >= 1);
        assert_eq!(f.json()["hooks"]["Stop"][0]["hooks"][0], foreign);
        assert_eq!(f.json()["hooks"]["Stop"][0]["timeout"], 9);
        assert_eq!(f.json()["hooks"]["Stop"][1]["hooks"], serde_json::json!([]));
        assert_eq!(f.json()["enabledPlugins"]["other@vendor"], true);
    }
    #[test]
    fn ambiguous_shell_substitutions_and_path_mentions_are_never_deleted() {
        let f = Fixture::new();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let mut data = f.json();
        let command = data["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();
        let foreign = vec![
            format!(
                "printf '%s' {}",
                quote(
                    &f.home
                        .join(".codex/skills/keito-time-track/hooks/dispatch.sh")
                        .display()
                        .to_string()
                )
            ),
            format!("{command} && audit"),
            command.replacen("KEITO_CLI_BIN=", "KEITO_CLI_BIN=$(audit) ", 1),
            format!("bash -c {}", quote(&command)),
        ];
        for c in &foreign {
            data["hooks"]["Stop"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"hooks":[{"type":"command","command":c}]}));
        }
        f.seed(data);
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        uninstall(&f.home, SkillAgent::Codex).unwrap();
        let got: Vec<_> = f.json()["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["hooks"][0]["command"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(got, foreign);
    }
    #[test]
    fn edited_discovery_or_generation_never_changes_configuration() {
        for edit_generation in [false, true] {
            let f = Fixture::new();
            let first = f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
            let target = if edit_generation {
                f.home
                    .join(".codex/.keito-capture-generations")
                    .join(first.generation)
            } else {
                f.home.join(".codex/skills/keito-time-track")
            };
            fs::write(target.join("SKILL.md"), "local edit").unwrap();
            let before = fs::read(f.config()).unwrap();
            assert!(f.install(SkillAgent::Codex, "0.167.0", false).is_err());
            assert_eq!(fs::read(f.config()).unwrap(), before);
            if !edit_generation {
                assert!(uninstall(&f.home, SkillAgent::Codex).is_err());
                assert_eq!(fs::read(f.config()).unwrap(), before);
            }
        }
    }
    #[test]
    fn damaged_executable_generation_is_not_reused_or_repaired() {
        let f = Fixture::new();
        let first = f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let target = f
            .home
            .join(".codex/.keito-capture-generations")
            .join(first.generation)
            .join("hooks/dispatch.sh");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let before = fs::read(f.config()).unwrap();
        assert!(f.install(SkillAgent::Codex, "0.167.0", false).is_err());
        assert_eq!(fs::read(f.config()).unwrap(), before);
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn uninstall_does_not_require_the_old_runtime_to_exist() {
        let f = Fixture::new();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        fs::remove_file(&f.runtime.core_path).unwrap();
        fs::remove_file(&f.runtime.guard_path).unwrap();
        assert!(
            uninstall(&f.home, SkillAgent::Codex)
                .unwrap()
                .payload_removed
        );
    }
    #[test]
    fn symlink_harness_settings_and_sources_are_refused() {
        for which in ["host", "skills", "config", "source"] {
            let f = Fixture::new();
            let outside = f.root.path().join("outside");
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("keep"), "unchanged").unwrap();
            match which {
                "host" => symlink(&outside, f.home.join(".codex")).unwrap(),
                "skills" => {
                    fs::create_dir(f.home.join(".codex")).unwrap();
                    symlink(&outside, f.home.join(".codex/skills")).unwrap();
                }
                "config" => {
                    fs::create_dir(f.home.join(".codex")).unwrap();
                    symlink(outside.join("keep"), f.config()).unwrap();
                }
                _ => symlink(outside.join("keep"), f.source.join("unexpected")).unwrap(),
            };
            assert!(f.install(SkillAgent::Codex, "0.167.0", false).is_err());
            assert_eq!(fs::read(outside.join("keep")).unwrap(), b"unchanged");
        }
    }
    #[test]
    fn malformed_settings_receipts_and_journals_are_sanitized() {
        let f = Fixture::new();
        f.seed(serde_json::json!({"hooks":{"Stop":"PRIVATE_SECRET"}}));
        let before = fs::read(f.config()).unwrap();
        let e = f
            .install(SkillAgent::Codex, "0.167.0", false)
            .unwrap_err()
            .to_string();
        assert!(!e.contains("PRIVATE_SECRET"));
        assert!(!e.contains(&f.home.display().to_string()));
        assert_eq!(fs::read(f.config()).unwrap(), before);
        f.seed(serde_json::json!({}));
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let receipt = f
            .home
            .join(".codex/skills/keito-time-track/capture-install.json");
        for bad in [b"[]".as_slice(), b"null", b"{\"files\":[]}"] {
            fs::write(&receipt, bad).unwrap();
            let before = fs::read(f.config()).unwrap();
            assert!(uninstall(&f.home, SkillAgent::Codex).is_err());
            assert_eq!(fs::read(f.config()).unwrap(), before);
        }
    }
    #[test]
    fn legacy_plugin_migration_requires_confirmation_and_preserves_foreign_plugin() {
        let f = Fixture::new();
        let host = f.home.join(".claude");
        fs::create_dir(&host).unwrap();
        let config = host.join("settings.json");
        fs::write(
            &config,
            br#"{"enabledPlugins":{"keito-time-track@keito":true,"other@vendor":true}}"#,
        )
        .unwrap();
        let before = fs::read(&config).unwrap();
        assert!(f.install(SkillAgent::ClaudeCode, "2.1.319", false).is_err());
        assert_eq!(fs::read(&config).unwrap(), before);
        assert!(
            f.install(SkillAgent::ClaudeCode, "2.1.319", true)
                .unwrap()
                .migrated
        );
        let data: Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
        assert_eq!(data["enabledPlugins"]["keito-time-track@keito"], false);
        assert_eq!(data["enabledPlugins"]["other@vendor"], true);
    }
    #[test]
    fn late_settings_edit_after_journal_fsync_is_preserved() {
        let f = Fixture::new();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let target = f.config();
        set_hook(Some(Box::new(move |p| {
            if p == "install-journal-fsynced" {
                fs::write(&target, b"{\"external\":true}").unwrap();
            }
        })));
        let result = f.install(SkillAgent::Codex, "0.167.1", false);
        set_hook(None);
        assert!(result.is_err());
        assert_eq!(fs::read(f.config()).unwrap(), b"{\"external\":true}");
        assert!(f
            .home
            .join(".codex/skills/keito-time-track/SKILL.md")
            .exists());
    }
    #[test]
    fn generation_changes_with_runtime_and_rejects_python_payload() {
        let mut f = Fixture::new();
        let first = f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        fs::write(&f.runtime.guard_path, b"replacement guard").unwrap();
        f.runtime.guard_sha256 = digest(b"replacement guard");
        assert_ne!(
            first.generation,
            f.install(SkillAgent::Codex, "0.167.0", false)
                .unwrap()
                .generation
        );
        fs::write(f.source.join("runtime.py"), b"not allowed").unwrap();
        assert!(f.install(SkillAgent::Codex, "0.167.0", false).is_err());
    }
    #[test]
    fn invalid_version_is_not_silently_truncated() {
        let f = Fixture::new();
        assert!(f
            .install(SkillAgent::Codex, "0.167.0+build", false)
            .is_err());
        assert!(!f.config().exists());
    }
    #[test]
    fn literal_parser_observes_posix_double_quote_backslashes() {
        assert_eq!(
            words(r#"KEITO_CLI_BIN="/safe/a\q" '/safe/dispatch.sh' --agent codex"#).unwrap()[0],
            r"KEITO_CLI_BIN=/safe/a\q"
        );
        assert_eq!(
            words(r#"'/safe/a'"'"'b$literal'"#).unwrap(),
            vec!["/safe/a'b$literal"]
        );
        for command in [
            "echo $(audit)",
            "echo `audit`",
            "x ; y",
            "x\ny",
            r#"echo "$(audit)""#,
            "echo ~root",
            "echo *",
        ] {
            assert!(words(command).is_none(), "{command}");
        }
    }
    #[test]
    fn legacy_grouped_hook_requires_explicit_migration_and_retains_backup() {
        let f = Fixture::new();
        let dest = f.home.join(".codex/skills/keito-time-track");
        fs::create_dir_all(dest.join("hooks")).unwrap();
        fs::write(dest.join("private-note"), b"legacy content").unwrap();
        let old = format!(
            "KEITO_AGENT_TYPE=codex {}",
            quote(dest.join("hooks/session-end.sh").to_str().unwrap())
        );
        let foreign = serde_json::json!({"type":"command","command":"audit"});
        f.seed(serde_json::json!({"hooks":{"Stop":[{"matcher":"keep","hooks":[{"type":"command","command":old},foreign.clone()]}]}}));
        let original = fs::read(f.config()).unwrap();
        assert!(f.install(SkillAgent::Codex, "0.167.0", false).is_err());
        assert_eq!(fs::read(f.config()).unwrap(), original);
        assert!(
            f.install(SkillAgent::Codex, "0.167.0", true)
                .unwrap()
                .migrated
        );
        assert_eq!(
            f.json()["hooks"]["Stop"][0]["hooks"],
            serde_json::json!([foreign])
        );
        let backups: Vec<_> = fs::read_dir(dest.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(".keito-time-track-backup-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            fs::read(backups[0].join("private-note")).unwrap(),
            b"legacy content"
        );
    }
    #[test]
    fn legacy_receipt_can_uninstall_but_cannot_be_reused_as_native_generation() {
        let f = Fixture::new();
        let installed = f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let receipt = f
            .home
            .join(".codex/skills/keito-time-track/capture-install.json");
        let mut v: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
        v["adapter_contract"] = 1.into();
        v.as_object_mut().unwrap().remove("runtime");
        fs::write(&receipt, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(uninstall(&f.home, SkillAgent::Codex).is_ok());
        let generation = f
            .home
            .join(".codex/.keito-capture-generations")
            .join(installed.generation)
            .join(RECEIPT);
        fs::write(generation, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(f.install(SkillAgent::Codex, "0.167.0", false).is_err());
    }
    #[test]
    fn malformed_or_symlinked_recovery_never_changes_settings() {
        for bad in [
            serde_json::json!([]),
            serde_json::json!({"schema_version":1,"stage":"../../outside","backup":"bad"}),
        ] {
            let f = Fixture::new();
            f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
            let config = fs::read(f.config()).unwrap();
            let journal = f.home.join(".codex").join(JOURNAL);
            fs::write(&journal, serde_json::to_vec(&bad).unwrap()).unwrap();
            assert!(uninstall(&f.home, SkillAgent::Codex).is_err());
            assert_eq!(fs::read(f.config()).unwrap(), config);
            assert!(journal.exists());
        }
    }
    #[test]
    fn final_config_temporary_fsync_edit_is_preserved() {
        let f = Fixture::new();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let target = f.config();
        set_hook(Some(Box::new(move |phase| {
            if phase == "install-config-fsynced" {
                fs::write(&target, b"{\"late\":true}").unwrap();
            }
        })));
        let result = f.install(SkillAgent::Codex, "0.167.1", false);
        set_hook(None);
        assert!(result.is_err());
        assert_eq!(fs::read(f.config()).unwrap(), b"{\"late\":true}");
    }
    #[test]
    fn concurrent_installer_refuses_before_configuration_mutation() {
        let f = Fixture::new();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let host = Dir::anchor(&f.home)
            .unwrap()
            .child(".codex", false)
            .unwrap();
        let _lock = host.lock("hooks.json.keito.lock").unwrap();
        let before = fs::read(f.config()).unwrap();
        assert!(f.install(SkillAgent::Codex, "0.167.1", false).is_err());
        assert!(uninstall(&f.home, SkillAgent::Codex).is_err());
        assert_eq!(fs::read(f.config()).unwrap(), before);
    }
    pub(crate) fn isolated_fixture_process(
        root: &Path,
        home: Option<&Path>,
        capture: Option<&Path>,
    ) -> Result<std::process::Command, &'static str> {
        let root = root
            .canonicalize()
            .map_err(|_| "fixture root unavailable")?;
        let home = home
            .ok_or("missing isolated home")?
            .canonicalize()
            .map_err(|_| "isolated home unavailable")?;
        let capture = capture
            .ok_or("missing isolated capture root")?
            .canonicalize()
            .map_err(|_| "isolated capture root unavailable")?;
        if home != root.join("home with ' literal $") || capture != home.join("capture") {
            return Err("foreign profile refused before spawn");
        }
        let executable = std::env::current_exe()
            .map_err(|_| "test executable unavailable")?
            .canonicalize()
            .map_err(|_| "test executable unavailable")?;
        let mut command = std::process::Command::new(executable);
        command
            .env("HOME", &home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("KEITO_CAPTURE_HOME", capture)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        Ok(command)
    }
    #[test]
    fn missing_or_foreign_home_refuses_before_constructing_mutation_process() {
        let f = Fixture::new();
        let capture = f.home.join("capture");
        assert!(isolated_fixture_process(f.root.path(), None, Some(&capture)).is_err());
        assert!(isolated_fixture_process(f.root.path(), Some(&f.home), None).is_err());
        let foreign = tempfile::tempdir().unwrap();
        assert!(
            isolated_fixture_process(f.root.path(), Some(foreign.path()), Some(&capture)).is_err()
        );
        assert!(
            isolated_fixture_process(f.root.path(), Some(&f.home), Some(foreign.path())).is_err()
        );
        assert!(isolated_fixture_process(f.root.path(), Some(&f.home), Some(&capture)).is_ok());
        assert!(!f.config().exists());
    }
    #[test]
    fn signal_fixture_child() {
        let Ok(root) = std::env::var("KEITO_TEST_NATIVE_ADAPTER_ROOT") else {
            return;
        };
        let root = PathBuf::from(root).canonicalize().unwrap();
        let isolated_home = root.join("home with ' literal $").canonicalize().unwrap();
        assert_eq!(
            std::env::var_os("HOME").map(PathBuf::from),
            Some(isolated_home.clone())
        );
        assert_eq!(
            std::env::var_os("KEITO_CAPTURE_HOME").map(PathBuf::from),
            Some(isolated_home.join("capture"))
        );
        let runtime = CaptureRuntimeIdentity {
            core_path: root.join("core"),
            guard_path: root.join("guard"),
            core_sha256: digest(b"synthetic core"),
            guard_sha256: digest(b"synthetic guard"),
            version: "0.2.0".into(),
            target: "aarch64-apple-darwin".into(),
            guard_contract: 1,
        };
        let boundary = std::env::var("KEITO_TEST_NATIVE_BOUNDARY").unwrap();
        let signal = std::env::var("KEITO_TEST_NATIVE_SIGNAL")
            .unwrap()
            .parse::<i32>()
            .unwrap();
        set_hook(Some(Box::new(move |p| {
            if p == boundary {
                unsafe {
                    libc::kill(libc::getpid(), signal);
                }
            }
        })));
        install(InstallOptions {
            source_root: &root.join("source"),
            profile_home: &root.join("home with ' literal $"),
            agent: SkillAgent::Codex,
            runtime: &runtime,
            confirm_migration: false,
            harness_version: Some("0.167.1"),
        })
        .unwrap();
        panic!("fault boundary was not reached");
    }
    #[test]
    fn real_process_death_preserves_registered_generations_and_recovers_discovery() {
        use std::os::unix::process::ExitStatusExt;
        for boundary in [
            "install-generation-published",
            "install-config-fsynced",
            "install-config-published",
            "install-payload-backup",
            "install-discovery-published",
        ] {
            for signal in [libc::SIGTERM, libc::SIGKILL] {
                let f = Fixture::new();
                f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
                let output = isolated_fixture_process(
                    f.root.path(),
                    Some(&f.home),
                    Some(&f.home.join("capture")),
                )
                .unwrap()
                .args([
                    "--exact",
                    "skill_adapter::implementation::tests::signal_fixture_child",
                    "--nocapture",
                ])
                .env(
                    "KEITO_TEST_NATIVE_ADAPTER_ROOT",
                    f.root.path().canonicalize().unwrap(),
                )
                .env("KEITO_TEST_NATIVE_BOUNDARY", boundary)
                .env("KEITO_TEST_NATIVE_SIGNAL", signal.to_string())
                .output()
                .unwrap();
                assert_eq!(output.status.signal(), Some(signal), "boundary {boundary}");
                for groups in f.json()["hooks"].as_object().unwrap().values() {
                    for group in groups.as_array().unwrap() {
                        for hook in group["hooks"].as_array().unwrap() {
                            let argv = words(hook["command"].as_str().unwrap()).unwrap();
                            let executable = argv
                                .iter()
                                .find(|s| s.ends_with("/hooks/dispatch.sh"))
                                .unwrap();
                            assert!(
                                Path::new(executable).is_file(),
                                "registered immutable payload lost at {boundary}"
                            );
                        }
                    }
                }
                f.install(SkillAgent::Codex, "0.167.1", false).unwrap();
                assert!(f
                    .home
                    .join(".codex/skills/keito-time-track/SKILL.md")
                    .is_file());
            }
        }
    }
    #[test]
    fn edited_discovery_after_journal_fsync_is_preserved_before_settings_switch() {
        let f = Fixture::new();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let before = fs::read(f.config()).unwrap();
        let target = f.home.join(".codex/skills/keito-time-track/SKILL.md");
        let edited = target.clone();
        set_hook(Some(Box::new(move |p| {
            if p == "install-journal-fsynced" {
                fs::write(&edited, b"late user edit").unwrap();
            }
        })));
        let result = f.install(SkillAgent::Codex, "0.167.1", false);
        set_hook(None);
        assert!(result.is_err());
        assert_eq!(fs::read(f.config()).unwrap(), before);
        assert_eq!(fs::read(target).unwrap(), b"late user edit");
    }
    #[test]
    fn backup_modified_after_move_is_never_deleted_by_install_or_uninstall() {
        for uninstalling in [false, true] {
            let f = Fixture::new();
            f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
            let skills = f.home.join(".codex/skills");
            let scan = skills.clone();
            set_hook(Some(Box::new(move |p| {
                if p == if uninstalling {
                    "uninstall-payload-backup"
                } else {
                    "install-payload-backup"
                } {
                    let backup = fs::read_dir(&scan)
                        .unwrap()
                        .map(|e| e.unwrap().path())
                        .find(|p| {
                            p.file_name()
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .starts_with(".keito-time-track-backup-")
                        })
                        .unwrap();
                    fs::write(backup.join("SKILL.md"), b"edited after move").unwrap();
                }
            })));
            let failed = if uninstalling {
                uninstall(&f.home, SkillAgent::Codex).is_err()
            } else {
                f.install(SkillAgent::Codex, "0.167.1", false).is_err()
            };
            set_hook(None);
            assert!(failed);
            assert!(fs::read_dir(skills)
                .unwrap()
                .map(|e| e.unwrap().path())
                .any(|p| fs::read(p.join("SKILL.md")).ok().as_deref()
                    == Some(b"edited after move".as_slice())));
        }
    }
    #[test]
    fn ordinary_config_io_failure_preserves_prior_discovery_and_settings() {
        let f = Fixture::new();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let before = fs::read(f.config()).unwrap();
        let payload = f.home.join(".codex/skills/keito-time-track/SKILL.md");
        let original = fs::read(&payload).unwrap();
        crate::adapter_files::set_publication_fault(Some("install-config-fsynced"));
        let result = f.install(SkillAgent::Codex, "0.167.1", false);
        crate::adapter_files::set_publication_fault(None);
        assert!(result.is_err());
        assert_eq!(fs::read(f.config()).unwrap(), before);
        assert_eq!(fs::read(payload).unwrap(), original);
        assert!(!f.home.join(".codex").join(JOURNAL).exists());
    }
    #[test]
    fn selected_target_preserves_other_harness_and_codex_feature_trust_bytes() {
        let f = Fixture::new();
        fs::create_dir_all(f.home.join(".codex")).unwrap();
        fs::create_dir_all(f.home.join(".claude")).unwrap();
        let config = f.home.join(".codex/config.toml");
        let other = f.home.join(".claude/settings.json");
        let trust =
            b"[features]\nhooks=false\n[hooks.state.synthetic]\ntrusted_hash='sha256:synthetic'\n";
        let foreign = b"{\"preserve\":\"other harness\"}";
        fs::write(&config, trust).unwrap();
        fs::write(&other, foreign).unwrap();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        uninstall(&f.home, SkillAgent::Codex).unwrap();
        assert_eq!(fs::read(config).unwrap(), trust);
        assert_eq!(fs::read(other).unwrap(), foreign);
        assert!(!f.home.join(".claude/skills").exists());
    }
    #[test]
    fn deep_recovery_metadata_is_sanitized_and_keeps_original_configuration() {
        let f = Fixture::new();
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let before = fs::read(f.config()).unwrap();
        let nested = format!("{}0{}", "[".repeat(1200), "]".repeat(1200));
        let journal = f.home.join(".codex").join(JOURNAL);
        fs::write(&journal, &nested).unwrap();
        let error = uninstall(&f.home, SkillAgent::Codex)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(&f.home.display().to_string()));
        assert_eq!(fs::read(f.config()).unwrap(), before);
        assert_eq!(fs::read_to_string(journal).unwrap(), nested);
    }
    #[test]
    fn real_bundled_native_payload_installs_without_python_or_developer_tests() {
        let f = Fixture::new();
        let this = Path::new(file!());
        let root = if this.is_absolute() {
            this.parent().unwrap().parent().unwrap().to_path_buf()
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        };
        let source = root.join("skills/keito-time-track");
        assert!(source.is_dir(), "the actual CLI runtime payload must exist");
        let out = install(InstallOptions {
            source_root: &source,
            profile_home: &f.home,
            agent: SkillAgent::Codex,
            runtime: &f.runtime,
            confirm_migration: false,
            harness_version: Some("0.167.0"),
        })
        .unwrap();
        let installed = Dir::anchor(&f.home)
            .unwrap()
            .child(".codex", false)
            .unwrap()
            .child(".keito-capture-generations", false)
            .unwrap()
            .child(&out.generation, false)
            .unwrap();
        let receipt = validate_receipt(&installed).unwrap();
        assert_eq!(receipt.files.len(), 21);
        assert!(receipt
            .files
            .keys()
            .all(|p| !p.ends_with(".py") && !p.starts_with("tests/")));
        assert_eq!(
            installed.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    #[test]
    fn inspect_requires_exact_current_native_pair_without_executing_it() {
        let f = Fixture::new();
        assert!(!inspect(&f.home, SkillAgent::Codex, None).unwrap());
        f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let config = fs::read(f.config()).unwrap();
        assert!(inspect(&f.home, SkillAgent::Codex, Some(&f.runtime)).unwrap());
        assert!(!inspect(&f.home, SkillAgent::Codex, None).unwrap());
        fs::write(&f.runtime.guard_path, b"changed image").unwrap();
        assert!(!inspect(&f.home, SkillAgent::Codex, Some(&f.runtime)).unwrap());
        assert_eq!(fs::read(f.config()).unwrap(), config);
    }
    #[test]
    fn inspect_refuses_missing_duplicate_or_changed_native_registration() {
        for case in [
            "missing",
            "extra",
            "timeout",
            "wrong-image",
            "matcher",
            "missing-core-hash",
            "wrong-core-hash",
        ] {
            let f = Fixture::new();
            f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
            let mut data = f.json();
            match case {
                "missing" => {
                    data["hooks"].as_object_mut().unwrap().remove("Stop");
                }
                "extra" => {
                    let duplicate = data["hooks"]["Stop"][0].clone();
                    data["hooks"]["Stop"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                "timeout" => data["hooks"]["Stop"][0]["hooks"][0]["timeout"] = 2.into(),
                "missing-core-hash" => {
                    let old = data["hooks"]["Stop"][0]["hooks"][0]["command"]
                        .as_str()
                        .unwrap();
                    data["hooks"]["Stop"][0]["hooks"][0]["command"] = old
                        .replace(
                            &format!("KEITO_CAPTURE_CORE_SHA256={} ", f.runtime.core_sha256),
                            "",
                        )
                        .into();
                }
                "wrong-core-hash" => {
                    let old = data["hooks"]["Stop"][0]["hooks"][0]["command"]
                        .as_str()
                        .unwrap();
                    data["hooks"]["Stop"][0]["hooks"][0]["command"] = old
                        .replace(
                            &format!("KEITO_CAPTURE_CORE_SHA256={}", f.runtime.core_sha256),
                            &format!("KEITO_CAPTURE_CORE_SHA256={}", "0".repeat(64)),
                        )
                        .into();
                }
                "wrong-image" => {
                    let old = data["hooks"]["Stop"][0]["hooks"][0]["command"]
                        .as_str()
                        .unwrap();
                    data["hooks"]["Stop"][0]["hooks"][0]["command"] = old
                        .replace("KEITO_CAPTURE_GUARD_BIN=", "UNRECOGNIZED_GUARD=")
                        .into();
                }
                _ => data["hooks"]["Stop"][0]["matcher"] = "never".into(),
            }
            f.seed(data);
            assert!(
                !inspect(&f.home, SkillAgent::Codex, Some(&f.runtime)).unwrap(),
                "{case}"
            );
        }
    }
    #[test]
    fn inspect_refuses_changed_payload_generation_and_legacy_receipts() {
        for case in ["payload", "generation", "legacy"] {
            let f = Fixture::new();
            let installed = f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
            let dest = f.home.join(".codex/skills/keito-time-track");
            match case {
                "payload" => fs::write(dest.join("SKILL.md"), b"edit").unwrap(),
                "generation" => fs::set_permissions(
                    f.home
                        .join(".codex/.keito-capture-generations")
                        .join(installed.generation)
                        .join("hooks/dispatch.sh"),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap(),
                _ => {
                    let receipt = dest.join(RECEIPT);
                    let mut data: Value =
                        serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
                    data["adapter_contract"] = 1.into();
                    data.as_object_mut().unwrap().remove("runtime");
                    fs::write(receipt, serde_json::to_vec(&data).unwrap()).unwrap();
                }
            }
            assert!(
                !inspect(&f.home, SkillAgent::Codex, Some(&f.runtime)).unwrap(),
                "{case}"
            );
        }
    }
    #[test]
    fn inspect_does_not_accept_a_rehashed_python_payload_as_native_contract_two() {
        let f = Fixture::new();
        let installed = f.install(SkillAgent::Codex, "0.167.0", false).unwrap();
        let dest = f.home.join(".codex/skills/keito-time-track");
        let generations = f.home.join(".codex/.keito-capture-generations");
        let generation = generations.join(&installed.generation);
        let mut receipt: Value =
            serde_json::from_slice(&fs::read(dest.join(RECEIPT)).unwrap()).unwrap();
        receipt["files"]["forged-runtime.py"] = digest(b"never executed").into();
        let files: BTreeMap<String, String> =
            serde_json::from_value(receipt["files"].clone()).unwrap();
        let new_key = generation_key(
            "codex",
            "0.167.0",
            &files,
            &serde_json::to_value(&f.runtime).unwrap(),
        )
        .unwrap();
        receipt["generation"] = new_key.clone().into();
        for folder in [&dest, &generation] {
            fs::write(folder.join("forged-runtime.py"), b"never executed").unwrap();
            fs::write(folder.join(RECEIPT), serde_json::to_vec(&receipt).unwrap()).unwrap();
        }
        fs::rename(&generation, generations.join(&new_key)).unwrap();
        let config = fs::read_to_string(f.config())
            .unwrap()
            .replace(&installed.generation, &new_key);
        fs::write(f.config(), config).unwrap();
        assert!(!inspect(&f.home, SkillAgent::Codex, Some(&f.runtime)).unwrap());
    }
}
