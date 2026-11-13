//! Cargo-linked inert SOURCE-FIXTURE native ancestor, never a real harness.
//! The two Cargo entrypoints bind their own literal supported version.
#[path = "../support/mutation_environment.rs"]
mod isolation;
use keito_cli::process_ownership::{
    io::IoPlan,
    protocol::ProtocolError,
    scope::{Scope, SlotId},
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
fn error() -> io::Error {
    io::Error::other("OWNED_SOURCE_FIXTURE")
}
fn owned(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().expect("owned fixture path unavailable");
    assert_eq!(canonical, path, "owned fixture aliases are unsupported");
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
    assert_eq!(metadata.mode() & 0o777, 0o700);
    canonical
}
fn bounded(path: &Path, root: &Path, limit: u64) -> Vec<u8> {
    assert!(path.starts_with(root));
    assert_eq!(path.canonicalize().unwrap(), path);
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
    assert_eq!(metadata.nlink(), 1);
    assert!(metadata.len() <= limit);
    let mut bytes = Vec::new();
    fs::File::open(path)
        .unwrap()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() as u64 <= limit);
    bytes
}
fn environment(command: &mut Command, case: &Path, home: &Path, capture: &Path) {
    command
        .env_clear()
        .current_dir(owned(&case.join("repo")))
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("KEITO_CAPTURE_HOME", capture)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("APPDATA", home.join("AppData"))
        .env("TMPDIR", owned(&case.join("tmp")))
        .env("PATH", owned(&case.join("bin")))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    #[cfg(target_os = "macos")]
    command.env(
        "__CF_USER_TEXT_ENCODING",
        format!("0x{:X}:0:0", unsafe { libc::getuid() }),
    );
}
fn complete(scope: &mut Scope, slot: &SlotId) -> io::Result<ExitStatus> {
    loop {
        scope.poll();
        if scope.slot_complete(slot) {
            return scope.slot_status(slot).ok_or_else(error);
        }
        if scope.failed() {
            return Err(error());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpaqueDiagnostic {
    NoOp,
    GitDirect,
    GitShim,
}
impl OpaqueDiagnostic {
    fn parse(value: &std::ffi::OsStr) -> io::Result<Self> {
        match value.to_str() {
            Some("no-op") => Ok(Self::NoOp),
            Some("git-direct") => Ok(Self::GitDirect),
            Some("git-shim") => Ok(Self::GitShim),
            _ => Err(error()),
        }
    }
    fn code(self) -> u8 {
        match self {
            Self::NoOp => 1,
            Self::GitDirect => 2,
            Self::GitShim => 3,
        }
    }
    fn command(self) -> Command {
        let mut command = Command::new(match self {
            Self::NoOp => "/usr/bin/true",
            Self::GitDirect => "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
            Self::GitShim => "/usr/bin/git",
        });
        if self != Self::NoOp {
            command.args(["init", "--quiet"]);
        }
        command
    }
}
fn diagnostic_clock() -> io::Result<u64> {
    let mut value = unsafe { std::mem::zeroed::<libc::timespec>() };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } != 0
        || !(0..1_000_000_000).contains(&value.tv_nsec)
    {
        return Err(error());
    }
    u64::try_from(value.tv_sec)
        .ok()
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|s| s.checked_add(value.tv_nsec as u64))
        .ok_or_else(error)
}
fn opaque_diagnostic(
    scope: &mut Scope,
    fields: &[std::ffi::OsString],
    case: &Path,
    home: &Path,
    capture: &Path,
    core: &Path,
) -> io::Result<i32> {
    if fields.len() != 4 {
        return Err(error());
    }
    // No caller-selected executable, argv, environment or timeout.
    let selected = OpaqueDiagnostic::parse(&fields[3])?;
    let mut command = selected.command();
    environment(&mut command, case, home, capture);
    let before = diagnostic_clock()?;
    let prior_end = scope.hard_end();
    let result = keito_cli::source_qa_scope::run_fixture_opaque(
        scope,
        command,
        IoPlan::new(Vec::new(), 8192, 8192)?,
        core,
    );
    let receipt = (|| -> io::Result<()> {
        use std::os::unix::process::ExitStatusExt;
        let after = diagnostic_clock()?;
        let report = scope.report();
        let slots: Vec<_> = report
            .slots
            .iter()
            .map(|slot| {
                serde_json::json!({
                    "slot":slot.slot, "status_code":slot.status.and_then(|s| s.code()),
                    "status_signal":slot.status.and_then(|s| s.signal()),
                    "status_raw":slot.status.map(|s| s.into_raw()),
                    "waits_known":slot.waits_known, "group_absent":slot.group_absent,
                    "terminal_eof":slot.terminal_eof, "io_required":slot.io_required,
                    "stdin_closed":slot.io.stdin_closed, "stdout_eof":slot.io.stdout_eof,
                    "stderr_eof":slot.io.stderr_eof, "accepted_at":slot.accepted_at,
                    "completion_end":slot.completion_end, "first_fault":slot.first_fault,
                })
            })
            .collect();
        let value = serde_json::json!({"diagnostic_only":true, "selector_code":selected.code(),
            "before_ns":before, "after_ns":after, "hard_end_before_ns":prior_end,
            "hard_end_after_ns":scope.hard_end(), "operation_ok":result.is_ok(),
            "first_fault":report.first_fault.map(|s| s.0), "finish_called":report.finish_called,
            "all_resources":report.all_resources, "clean":report.clean, "slots":slots});
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(case.join("opaque-diagnostic.json"))?;
        serde_json::to_writer(&mut file, &value)?;
        file.write_all(b"\n")?;
        file.sync_all()
    })();
    match result {
        Err(primary) => {
            let _ = receipt;
            Err(primary)
        }
        Ok(value) => {
            receipt?;
            Ok(value.status.code().unwrap_or(125))
        }
    }
}
fn hook_probe_diagnostic(scope: &mut Scope, case: &Path, core: &Path) -> io::Result<i32> {
    let mut count = 0u8;
    let entry_end = scope.hard_end();
    let result = keito_cli::source_qa_scope::diagnose_fixture_hook_probes(
        scope,
        core,
        &case.join("repo"),
        |ordinal, before, after, operation_ok, report| {
            if count >= 4 || ordinal != count + 1 {
                return Err(error());
            }
            count = ordinal;
            let slots: Vec<_> = report
                .slots
                .iter()
                .map(|slot| {
                    serde_json::json!({
                        "slot":slot.slot, "status":slot.status.and_then(|s| s.code()),
                        "waits_known":slot.waits_known, "group_absent":slot.group_absent,
                        "terminal_eof":slot.terminal_eof, "io_required":slot.io_required,
                        "stdin_closed":slot.io.stdin_closed, "stdout_eof":slot.io.stdout_eof,
                        "stderr_eof":slot.io.stderr_eof, "accepted_at":slot.accepted_at,
                        "completion_end":slot.completion_end, "first_fault":slot.first_fault,
                    })
                })
                .collect();
            let value = serde_json::json!({"diagnostic_only":true, "ordinal":ordinal,
                "before_ns":before, "after_ns":after, "operation_ok":operation_ok,
                "entry_end_ns":entry_end,
                "first_fault":report.first_fault.map(|s| s.0),
                "finish_called":report.finish_called, "all_resources":report.all_resources,
                "clean":report.clean, "slots":slots});
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(case.join(format!("hook-probe-{ordinal:02}.json")))?;
            serde_json::to_writer(&mut output, &value)?;
            output.write_all(b"\n")?;
            output.sync_all()
        },
    );
    let report = scope.report();
    let summary = serde_json::json!({"diagnostic_only":true, "probes_observed":count,
        "operation_ok":result.is_ok(), "first_fault":report.first_fault.map(|s| s.0),
        "hard_end_ns":scope.hard_end(), "finish_called":report.finish_called,
        "all_resources":report.all_resources, "clean":report.clean});
    let saved = (|| -> io::Result<()> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(case.join("hook-probe-summary.json"))?;
        serde_json::to_writer(&mut output, &summary)?;
        output.write_all(b"\n")?;
        output.sync_all()
    })();
    if saved.is_err() {
        scope.begin_cancel(ProtocolError("QA_PROBE_SUMMARY"), scope.hard_end());
    }
    match result {
        Err(_) => Err(error()),
        Ok(()) => {
            saved?;
            Ok(0)
        }
    }
}
// Developer-only failure diagnosis. Never serialize argv, environment, command
// output or arbitrary io::Error text; keep the original operation error primary.
fn failure_receipt(
    arguments: &[std::ffi::OsString],
    scope: &Scope,
    finality: bool,
) -> io::Result<()> {
    let mode = arguments
        .first()
        .and_then(|s| s.to_str())
        .ok_or_else(error)?;
    let offset = match mode {
        "--chain" => 2,
        "--cli" | "--run" | "--git-init" | "--opaque-diagnostic" | "--hook-probe-diagnostic" => 1,
        _ => return Err(error()),
    };
    let fields = arguments.get(offset..offset + 3).ok_or_else(error)?;
    let root = Path::new(&fields[0]);
    let case = Path::new(&fields[1]);
    let home = case.join("home");
    let capture = home.join("capture");
    isolation::checked(
        root,
        case,
        Some(&home),
        Some(&capture),
        Some(Path::new(&fields[2])),
    )
    .map_err(|_| error())?;
    let report = scope.report();
    let slots: Vec<_> = report
        .slots
        .iter()
        .map(|slot| {
            serde_json::json!({
                "slot": slot.slot, "status": slot.status.and_then(|status| status.code()),
                "waits_known": slot.waits_known, "group_absent": slot.group_absent,
                "terminal_eof": slot.terminal_eof, "io_required": slot.io_required,
                "stdin_closed": slot.io.stdin_closed, "stdout_eof": slot.io.stdout_eof,
                "stderr_eof": slot.io.stderr_eof, "accepted_at": slot.accepted_at,
                "completion_end": slot.completion_end, "first_fault": slot.first_fault,
            })
        })
        .collect();
    let value = serde_json::json!({"source_fixture_diagnosis_only": true,
        "late_final_facts": finality, "first_fault": report.first_fault.map(|fault| fault.0),
        "hard_end_ns": scope.hard_end(), "finish_called": report.finish_called,
        "all_resources": report.all_resources, "clean": report.clean, "slots": slots});
    let name = if finality {
        "native-owner-FINAL.json"
    } else {
        "native-owner-FIRST.json"
    };
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(case.join(name))?;
    serde_json::to_writer(&mut output, &value)?;
    output.write_all(b"\n")?;
    output.sync_all()
}
fn execute(scope: &mut Scope, arguments: &[std::ffi::OsString]) -> io::Result<i32> {
    let mode = arguments
        .first()
        .and_then(|s| s.to_str())
        .ok_or_else(error)?;
    let chain = mode == "--chain";
    let depth = if chain {
        assert_eq!(arguments.len(), 8);
        let depth = arguments[1].to_str().unwrap().parse::<usize>().unwrap();
        assert!(depth <= 8);
        Some(depth)
    } else {
        None
    };
    let fields = if chain {
        &arguments[2..]
    } else {
        &arguments[1..]
    };
    assert!(matches!(
        mode,
        "--cli"
            | "--run"
            | "--chain"
            | "--git-init"
            | "--opaque-diagnostic"
            | "--hook-probe-diagnostic"
    ));
    assert!(fields.len() >= 3);
    let root = owned(Path::new(&fields[0]));
    let case = owned(Path::new(&fields[1]));
    assert!(case.starts_with(&root));
    let home = owned(&case.join("home"));
    let capture = owned(&home.join("capture"));
    let core = PathBuf::from(&fields[2]);
    let marker = case.join("constructor-must-not-run");
    for absent in [0, 1, 2] {
        let invalid = isolation::checked(
            &root,
            &case,
            (absent != 0).then_some(home.as_path()),
            (absent != 1).then_some(capture.as_path()),
            (absent != 2).then_some(core.as_path()),
        );
        if invalid.is_ok() {
            fs::write(&marker, b"unsafe").unwrap();
        }
        assert!(!marker.exists());
    }
    let checked =
        isolation::checked(&root, &case, Some(&home), Some(&capture), Some(&core)).unwrap();
    assert_eq!(checked.core, core);
    assert_eq!(checked.home, home);
    assert_eq!(checked.capture, capture);
    for relative in [".codex", ".claude", "config", "state", "AppData"] {
        owned(&home.join(relative));
    }
    if mode == "--hook-probe-diagnostic" {
        assert_eq!(fields.len(), 3);
        return hook_probe_diagnostic(scope, &case, &core);
    }
    if mode == "--opaque-diagnostic" {
        return opaque_diagnostic(scope, fields, &case, &home, &capture, &core);
    }
    if mode == "--git-init" {
        assert_eq!(fields.len(), 3);
        let mut command = Command::new(if cfg!(target_os = "macos") {
            "/Applications/Xcode.app/Contents/Developer/usr/bin/git"
        } else {
            "/usr/bin/git"
        });
        environment(&mut command, &case, &home, &capture);
        command.args(["init", "--quiet"]);
        let result = keito_cli::source_qa_scope::run_fixture_opaque(
            scope,
            command,
            IoPlan::new(Vec::new(), 8192, 8192)?,
            &core,
        )?;
        return Ok(result.status.code().unwrap_or(125));
    }
    if mode == "--cli" {
        assert!(fields.len() >= 4);
        let mut command = Command::new(&checked.core);
        environment(&mut command, &case, &home, &capture);
        command
            .args(&fields[3..])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let child = scope.start_cooperative(command)?;
        return Ok(complete(scope, &child)?.code().unwrap_or(125));
    }
    assert_eq!(fields.len(), 6, "run root case core command event result");
    if let Some(depth) = depth.filter(|depth| *depth > 0) {
        let image = std::env::current_exe().unwrap();
        assert!(image.starts_with(&root) && image.canonicalize().unwrap() == image);
        let metadata = fs::symlink_metadata(&image).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink() && metadata.nlink() == 1);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        let mut next = Command::new(&image);
        environment(&mut next, &case, &home, &capture);
        next.arg("--chain")
            .arg((depth - 1).to_string())
            .args(fields)
            .stdin(Stdio::null());
        let child = scope.start_cooperative(next)?;
        return Ok(complete(scope, &child)?.code().unwrap_or(125));
    }
    let command = String::from_utf8(bounded(Path::new(&fields[3]), &case, 64 * 1024)).unwrap();
    let event = bounded(Path::new(&fields[4]), &case, 1024 * 1024);
    let result = PathBuf::from(&fields[5]);
    assert_eq!(result.parent().unwrap(), case);
    // Exclusive receipt failure occurs at actual completion, never pre-launch.
    let mut process = Command::new("/bin/bash");
    environment(&mut process, &case, &home, &capture);
    process.args(["-c", &command]);
    let started = Instant::now();
    let child = scope.start_cooperative_with_io(process, IoPlan::new(event, 0, 0)?)?;
    let status = complete(scope, &child)?;
    let resources = scope.finish().clean;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let value = serde_json::json!({"source_fixture_only":true,"exit_code":status.code(),
        "host_timeout":elapsed >= 1000.0,"elapsed_ms":elapsed,"resources_clean":resources});
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(result)?;
    serde_json::to_writer(&mut output, &value)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    if !resources || elapsed >= 1000.0 || !status.success() {
        return Err(error());
    }
    Ok(0)
}
pub(super) fn run(version: &'static str) -> i32 {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let hook = arguments
        .first()
        .is_some_and(|mode| mode == "--run" || mode == "--chain");
    let duration = if arguments
        .first()
        .is_some_and(|mode| mode == "--hook-probe-diagnostic")
    {
        500_000_000
    } else if hook {
        1_000_000_000
    } else {
        4_000_000_000
    };
    let mut scope = match Scope::entry_from_environment(duration) {
        Ok(Some(scope)) => scope,
        Ok(None) => {
            let mut clock = unsafe { std::mem::zeroed::<libc::timespec>() };
            if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock) } != 0 {
                return 1;
            }
            let at = (clock.tv_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(clock.tv_nsec as u64);
            match Scope::local_entry(at.saturating_add(duration)) {
                Ok(scope) => scope,
                Err(_) => return 1,
            }
        }
        Err(_) => return 1,
    };
    let result = if arguments.len() == 1 && arguments[0] == "--version" {
        println!("owned-inert-source-fixture {version}");
        Ok(0)
    } else {
        execute(&mut scope, &arguments)
    };
    if result.is_err() {
        let _ = failure_receipt(&arguments, &scope, false);
        scope.begin_cancel(
            ProtocolError("FIXTURE_OPERATION_OR_RECEIPT"),
            scope.hard_end(),
        );
    }
    let clean = scope.finish().clean && result.as_ref().is_ok_and(|code| *code == 0);
    if result.is_err() {
        let _ = failure_receipt(&arguments, &scope, true);
    }
    if !scope.publish_terminal(clean) {
        return 1;
    }
    result.unwrap_or(1)
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[test]
    fn opaque_diagnostic_selector_refuses_programs_and_extra_arguments() {
        for value in [
            "",
            "/bin/sh",
            "git",
            "git-direct --version",
            "git-shim;true",
            "NO-OP",
        ] {
            assert!(OpaqueDiagnostic::parse(std::ffi::OsStr::new(value)).is_err());
        }
        assert_eq!(
            OpaqueDiagnostic::parse(std::ffi::OsStr::new("no-op")).unwrap(),
            OpaqueDiagnostic::NoOp
        );
        assert_eq!(
            OpaqueDiagnostic::parse(std::ffi::OsStr::new("git-direct")).unwrap(),
            OpaqueDiagnostic::GitDirect
        );
        assert_eq!(
            OpaqueDiagnostic::parse(std::ffi::OsStr::new("git-shim")).unwrap(),
            OpaqueDiagnostic::GitShim
        );
    }
}
