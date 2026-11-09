//! Owned inert SOURCE-FIXTURE native ancestor. Never a real coding harness.
//! Compile with OWNED_SOURCE_FIXTURE_VERSION set to the fixed supported version.
#[path = "../support/mutation_environment.rs"]
mod isolation;

use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

unsafe extern "C" {
    fn geteuid() -> u32;
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}

fn owned(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().expect("owned fixture path unavailable");
    assert_eq!(canonical, path, "owned fixture aliases are unsupported");
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.uid(), unsafe { geteuid() });
    assert_eq!(metadata.mode() & 0o777, 0o700);
    canonical
}

fn bounded(path: &Path, root: &Path, limit: u64) -> Vec<u8> {
    assert!(path.starts_with(root));
    assert_eq!(path.canonicalize().unwrap(), path);
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.uid(), unsafe { geteuid() });
    assert_eq!(metadata.nlink(), 1);
    assert!(metadata.len() <= limit);
    let mut bytes = Vec::new();
    fs::File::open(path).unwrap().take(limit + 1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() as u64 <= limit);
    bytes
}

fn main() {
    let mut arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let chain_depth = if arguments.first().is_some_and(|mode| mode == "--chain") {
        assert_eq!(arguments.len(), 8);
        let depth=arguments[1].to_str().unwrap().parse::<usize>().unwrap();
        assert!(depth<=8);
        arguments.remove(1);arguments[0]="--run".into();Some(depth)
    } else { None };
    if arguments.len() == 1 && arguments[0] == "--version" {
        println!("owned-inert-source-fixture {}", env!("OWNED_SOURCE_FIXTURE_VERSION"));
        return;
    }
    if arguments.first().is_some_and(|mode| mode == "--cli") {
        assert!(arguments.len() >= 5);
        let root = owned(Path::new(&arguments[1]));
        let case = owned(Path::new(&arguments[2]));
        let home = owned(&case.join("home"));
        let capture = owned(&home.join("capture"));
        let core = PathBuf::from(&arguments[3]);
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
        let bin = owned(&case.join("bin"));
        let temp = owned(&case.join("tmp"));
        for relative in [".codex", ".claude", "config", "state", "AppData"] { owned(&home.join(relative)); }
        let mut command = Command::new(&checked.core);
        command.env_clear().current_dir(owned(&case.join("repo")))
            .env("HOME", checked.home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("KEITO_CAPTURE_HOME", checked.capture)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("APPDATA", home.join("AppData"))
            .env("TMPDIR", temp).env("PATH", bin)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(&arguments[4..]).stdin(Stdio::null());
        let status = command.status().unwrap();
        std::process::exit(status.code().unwrap_or(125));
    }
    assert_eq!(arguments.len(), 7, "--run <run-root> <case-root> <source-core> <registered-command-file> <event-file> <result-file>");
    assert_eq!(arguments[0], "--run");
    let root = owned(Path::new(&arguments[1]));
    let case = owned(Path::new(&arguments[2]));
    assert!(case.starts_with(&root));
    let home = owned(&case.join("home"));
    let capture = owned(&home.join("capture"));
    let core = PathBuf::from(&arguments[3]);
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
    if let Some(depth)=chain_depth.filter(|depth|*depth>0) {
        // Keep every inspected wrong-image ancestor inside an owned inert
        // chain. Do not walk into the QA runner, user shell or real harness.
        let image=std::env::current_exe().unwrap();
        assert!(image.starts_with(&root) && image.canonicalize().unwrap()==image);
        let metadata=fs::symlink_metadata(&image).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink() && metadata.nlink()==1);
        assert_eq!(metadata.uid(),unsafe {geteuid()});
        let mut next=Command::new(&image);
        next.env_clear().current_dir(&case)
            .env("HOME",checked.home).env("CODEX_HOME",home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR",home.join(".claude")).env("KEITO_CAPTURE_HOME",checked.capture)
            .env("XDG_CONFIG_HOME",home.join("config")).env("XDG_STATE_HOME",home.join("state"))
            .env("APPDATA",home.join("AppData")).env("TMPDIR",owned(&case.join("tmp")))
            .env("PATH",owned(&case.join("bin"))).env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_SYSTEM","/dev/null")
            .arg("--chain").arg((depth-1).to_string()).args(&arguments[1..]).stdin(Stdio::null());
        let mut child=next.spawn().unwrap();let status=child.wait().unwrap();
        std::process::exit(status.code().unwrap_or(125));
    }
    let command = String::from_utf8(bounded(Path::new(&arguments[4]), &case, 64 * 1024)).unwrap();
    let event = bounded(Path::new(&arguments[5]), &case, 1024 * 1024);
    let result = PathBuf::from(&arguments[6]);
    assert_eq!(result.parent().unwrap(), case);
    assert!(!result.exists() && !result.is_symlink());
    let bin = owned(&case.join("bin"));
    let temp = owned(&case.join("tmp"));
    for relative in [".codex", ".claude", "config", "state", "AppData"] { owned(&home.join(relative)); }
    // Command construction occurs only after explicit owned canonical checks
    // and the missing-operand negatives. The caller's single kernel policy is
    // inherited; this fixture never applies a nested sandbox or enables trust.
    let mut process = Command::new("/bin/bash");
    process.env_clear().current_dir(case.join("repo"))
        .env("HOME", checked.home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("KEITO_CAPTURE_HOME", checked.capture)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("APPDATA", home.join("AppData"))
        .env("TMPDIR", temp)
        .env("PATH", bin)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(["-c", &command])
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe { process.pre_exec(|| { if setpgid(0, 0) == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) } }); }
    let started = Instant::now();
    let mut child = process.spawn().unwrap();
    child.stdin.take().unwrap().write_all(&event).unwrap();
    let (status, timeout) = loop {
        if let Some(status) = child.try_wait().unwrap() { break (status, false); }
        if started.elapsed() >= Duration::from_secs(1) {
            unsafe { kill(-(child.id() as i32), 15); }
            std::thread::sleep(Duration::from_millis(25));
            unsafe { kill(-(child.id() as i32), 9); }
            break (child.wait().unwrap(), true);
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    // Content-free observation only. The fixture never logs event/config bytes.
    fs::write(result, format!("{{\"source_fixture_only\":true,\"exit_code\":{:?},\"host_timeout\":{},\"elapsed_ms\":{:.6}}}\n",
        status.code(), timeout, started.elapsed().as_secs_f64() * 1000.0).replace("Some(", "").replace(')', "").replace("None", "null")).unwrap();
    assert!(!timeout && status.success(), "owned source-fixture host ceiling/dispatch failed");
}
