//! Cargo-linked inert SOURCE-FIXTURE native ancestor, never a real harness.
//! The two Cargo entrypoints bind their own literal supported version.
#[path = "../support/mutation_environment.rs"]
mod isolation;
use keito_cli::process_ownership::{io::IoPlan, protocol::ProtocolError, scope::{Scope, SlotId}};
use std::{
    fs::{self, OpenOptions}, io::{self, Read, Write},
    os::unix::fs::MetadataExt, path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio}, time::{Duration, Instant},
};
fn error() -> io::Error { io::Error::other("OWNED_SOURCE_FIXTURE") }
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
    fs::File::open(path).unwrap().take(limit + 1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() as u64 <= limit);
    bytes
}
fn environment(command: &mut Command, case: &Path, home: &Path, capture: &Path) {
    command.env_clear().current_dir(owned(&case.join("repo")))
        .env("HOME", home).env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude")).env("KEITO_CAPTURE_HOME", capture)
        .env("XDG_CONFIG_HOME", home.join("config")).env("XDG_STATE_HOME", home.join("state"))
        .env("APPDATA", home.join("AppData")).env("TMPDIR", owned(&case.join("tmp")))
        .env("PATH", owned(&case.join("bin")))
        .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_SYSTEM", "/dev/null");
    #[cfg(target_os = "macos")]
    command.env("__CF_USER_TEXT_ENCODING", format!("0x{:X}:0:0", unsafe { libc::getuid() }));
}
fn complete(scope: &mut Scope, slot: &SlotId) -> io::Result<ExitStatus> {
    loop {
        scope.poll();
        if scope.slot_complete(slot) { return scope.slot_status(slot).ok_or_else(error); }
        if scope.failed() { return Err(error()); }
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn execute(scope: &mut Scope, arguments: &[std::ffi::OsString]) -> io::Result<i32> {
    let mode = arguments.first().and_then(|s| s.to_str()).ok_or_else(error)?;
    let chain = mode == "--chain";
    let depth = if chain {
        assert_eq!(arguments.len(), 8);
        let depth = arguments[1].to_str().unwrap().parse::<usize>().unwrap();
        assert!(depth <= 8); Some(depth)
    } else { None };
    let fields = if chain { &arguments[2..] } else { &arguments[1..] };
    assert!(matches!(mode, "--cli" | "--run" | "--chain" | "--git-init"));
    assert!(fields.len() >= 3);
    let root = owned(Path::new(&fields[0]));
    let case = owned(Path::new(&fields[1]));
    assert!(case.starts_with(&root));
    let home = owned(&case.join("home"));
    let capture = owned(&home.join("capture"));
    let core = PathBuf::from(&fields[2]);
    let marker = case.join("constructor-must-not-run");
    for absent in [0, 1, 2] {
        let invalid = isolation::checked(&root, &case,
            (absent != 0).then_some(home.as_path()),
            (absent != 1).then_some(capture.as_path()),
            (absent != 2).then_some(core.as_path()));
        if invalid.is_ok() { fs::write(&marker, b"unsafe").unwrap(); }
        assert!(!marker.exists());
    }
    let checked = isolation::checked(&root, &case, Some(&home), Some(&capture), Some(&core)).unwrap();
    assert_eq!(checked.core, core);
    for relative in [".codex", ".claude", "config", "state", "AppData"] { owned(&home.join(relative)); }
    if mode == "--git-init" {
        assert_eq!(fields.len(), 3);
        let mut command = Command::new("/usr/bin/git");
        environment(&mut command, &case, &home, &capture);
        command.args(["init", "--quiet"]);
        let result = keito_cli::source_qa_scope::run_fixture_opaque(scope, command, IoPlan::new(Vec::new(), 8192, 8192)?, &core)?;
        return Ok(result.status.code().unwrap_or(125));
    }
    if mode == "--cli" {
        assert!(fields.len() >= 4);
        let mut command = Command::new(&checked.core);
        environment(&mut command, &case, &home, &capture);
        command.args(&fields[3..]).stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
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
        next.arg("--chain").arg((depth - 1).to_string()).args(fields).stdin(Stdio::null());
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
    let mut output = OpenOptions::new().write(true).create_new(true).open(result)?;
    serde_json::to_writer(&mut output, &value)?;
    output.write_all(b"\n")?; output.sync_all()?;
    if !resources || elapsed >= 1000.0 || !status.success() { return Err(error()); }
    Ok(0)
}
pub(super) fn run(version: &'static str) -> i32 {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let hook = arguments.first().is_some_and(|mode| mode == "--run" || mode == "--chain");
    let duration = if hook { 1_000_000_000 } else { 4_000_000_000 };
    let mut scope = match Scope::entry_from_environment(duration) {
        Ok(Some(scope)) => scope,
        Ok(None) => {
            let mut clock = unsafe { std::mem::zeroed::<libc::timespec>() };
            if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock) } != 0 { return 1; }
            let at = (clock.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(clock.tv_nsec as u64);
            Scope::with_owner(Default::default(), at.saturating_add(duration))
        }
        Err(_) => return 1,
    };
    let result = if arguments.len() == 1 && arguments[0] == "--version" {
        println!("owned-inert-source-fixture {version}"); Ok(0)
    } else { execute(&mut scope, &arguments) };
    if result.is_err() { scope.begin_cancel(ProtocolError("FIXTURE_OPERATION_OR_RECEIPT"), scope.hard_end()); }
    let clean = scope.finish().clean && result.as_ref().is_ok_and(|code| *code == 0);
    if !scope.publish_terminal(clean) { return 1; }
    result.unwrap_or(1)
}
