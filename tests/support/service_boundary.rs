//! Test-only proof receipt plus current-process kernel-denial sentinel.
//! Direct macOS Cargo runs fail before any actual CLI observer can spawn.
use assert_cmd::Command;
use std::ffi::OsStr;
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(target_os = "macos")]
    {
        static VERIFIED: std::sync::OnceLock<Result<(), &'static str>> = std::sync::OnceLock::new();
        if let Err(reason) = VERIFIED.get_or_init(verify) {
            panic!("Use tests/run-macos-isolated.sh: {reason}");
        }
    }
    Command::new(program)
}
#[cfg(target_os = "macos")]
fn verify() -> Result<(), &'static str> {
    use sha2::{Digest, Sha256};
    use std::{fs, os::unix::fs::MetadataExt, path::PathBuf};
    let file = PathBuf::from(
        std::env::var_os("KEITO_TEST_SERVICE_BOUNDARY_RECEIPT")
            .ok_or("missing owned boundary receipt")?,
    );
    if file.file_name().and_then(|v| v.to_str()) != Some("service-boundary-receipt.json") {
        return Err("invalid receipt name");
    }
    let root = file.parent().ok_or("missing receipt root")?;
    if root.canonicalize().ok().as_deref() != Some(root) {
        return Err("noncanonical receipt root");
    }
    let m = fs::symlink_metadata(root).map_err(|_| "missing receipt root")?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o700 {
        return Err("foreign receipt root");
    }
    let m = fs::symlink_metadata(&file).map_err(|_| "missing receipt")?;
    if !m.is_file()
        || m.file_type().is_symlink()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
        || m.len() > 16384
    {
        return Err("unsafe receipt");
    }
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&file).map_err(|_| "unreadable receipt")?)
            .map_err(|_| "invalid receipt")?;
    if value["schema"] != 1
        || value["passed"] != true
        || value["actual_service_utility_executed"] != false
    {
        return Err("unproven receipt");
    }
    let digest = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>()
    };
    for (name, key) in [
        ("boundary.sb", "policy_sha256"),
        ("owned-checker", "checker_sha256"),
        ("blocked-native-marker", "blocked_sha256"),
    ] {
        let path = root.join(name);
        let m = fs::symlink_metadata(&path).map_err(|_| "missing proof input")?;
        if !m.is_file()
            || m.file_type().is_symlink()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o777 != if name == "boundary.sb" { 0o600 } else { 0o700 }
            || m.len() > 128 * 1024 * 1024
        {
            return Err("unsafe proof input");
        }
        if value[key].as_str()
            != Some(digest(&fs::read(path).map_err(|_| "unreadable proof input")?).as_str())
        {
            return Err("changed proof input");
        }
    }
    if value["helper_source_sha256"].as_str()
        != Some(digest(include_bytes!("service_boundary.rs")).as_str())
        || value["checker_source_sha256"].as_str()
            != Some(digest(include_bytes!("../../src/bin/check-service-boundary.rs")).as_str())
    {
        return Err("proof source changed");
    }
    // A stale receipt outside the inherited policy must never authorize a CLI.
    // Only this owned inert marker executable is probed, never launchctl.
    let marker = root.join("native.marker");
    if marker.exists() {
        return Err("prior unprotected sentinel execution");
    }
    let mut child = std::process::Command::new(root.join("blocked-native-marker"));
    let home = root.join("home");
    child
        .env_clear()
        .current_dir(root)
        .env("HOME", &home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("KEITO_CAPTURE_HOME", home.join("capture"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("AppData"))
        .env("PATH", root.join("empty-bin"))
        .arg("marker")
        .arg(root);
    let denied = child
        .output()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied);
    if !denied || marker.exists() {
        return Err("current process lacks inherited execution denial");
    }
    Ok(())
}
