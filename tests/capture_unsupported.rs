//! Windows has manual commands, not qualified automatic capture. Detect accidental
//! entry into auth/network/store/profile work before the unsupported boundary.
#![cfg(not(unix))]
use assert_cmd::Command;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use wiremock::MockServer;

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            assert!(!fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink());
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[tokio::test]
async fn unsupported_native_commands_never_read_auth_upload_acknowledge_or_change_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    let executable = root.join("keito.exe");
    fs::copy(assert_cmd::cargo::cargo_bin!("keito"), &executable).unwrap();
    assert_eq!(executable.canonicalize().unwrap(), executable);
    for relative in [
        ".codex",
        ".claude",
        "capture",
        "config/keito",
        "AppData/Roaming/keito",
        "empty-bin",
        "tmp",
    ] {
        fs::create_dir_all(home.join(relative)).unwrap();
    }
    // Loading this private malformed credential config would fail with a different
    // diagnostic. Existing unrelated profile bytes must survive every refusal.
    for relative in [
        "config/keito/config.toml",
        "AppData/Roaming/keito/config.toml",
    ] {
        fs::write(
            home.join(relative),
            "api_key = \"PRIVATE_AUTH_CANARY\" malformed\n",
        )
        .unwrap();
    }
    fs::write(home.join(".codex/hooks.json"), "{\"unrelated\":true}\n").unwrap();
    fs::write(home.join(".claude/settings.json"), "{\"unrelated\":true}\n").unwrap();
    let before = files(&home);
    let server = MockServer::start().await;
    let commands: Vec<Vec<&str>> = vec![
        vec!["status"],
        vec![
            "setup",
            "--agent",
            "codex",
            "--project",
            "p1",
            "--task",
            "t1",
            "--confirm",
        ],
        vec!["sync"],
        vec!["worker", "--interval-seconds", "1"],
        vec!["worker-status", "--all"],
        vec!["worker-uninstall", "--repo"],
        vec!["recover", "--confirm"],
        vec!["preview", "--session", "s1"],
        vec!["pause", "--repo"],
        vec!["resume", "--repo"],
        vec!["disable", "--repo"],
        vec!["disable-all"],
        vec![
            "hook",
            "--agent",
            "codex",
            "--event",
            "prompt",
            "--harness-version",
            "0.167.0",
        ],
        vec![
            "hook",
            "--agent",
            "claude-code",
            "--event",
            "prompt",
            "--harness-version",
            "2.1.319",
        ],
    ];
    for args in commands {
        let mut command = Command::new(&executable);
        command
            .env_clear()
            .current_dir(&root)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("APPDATA", home.join("AppData/Roaming"))
            .env("LOCALAPPDATA", home.join("AppData/Local"))
            .env("KEITO_CAPTURE_HOME", home.join("capture"))
            .env("PATH", home.join("empty-bin"))
            .env("TEMP", home.join("tmp"))
            .env("TMP", home.join("tmp"))
            .env("KEITO_API_URL", server.uri())
            .env("KEITO_API_KEY", "PRIVATE_AUTH_CANARY")
            .env("KEITO_ACCOUNT_ID", "workspace_fixture")
            .env("KEITO_CAPTURE_ACK_FD", "3");
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.args(["--json", "capture"]).args(&args);
        if args[0] == "hook" {
            command.write_stdin(serde_json::json!({"cwd":root,"session_id":"session_fixture","turn_id":"turn_fixture","prompt_id":"prompt_fixture","prompt":"PRIVATE_INPUT_CANARY"}).to_string());
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(
            output.stdout.is_empty(),
            "unexpected readiness or ACK: {args:?}"
        );
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains("Automatic capture is unsupported on this platform"),
            "{args:?}: {error}"
        );
        assert!(!error.contains("PRIVATE_") && !error.contains("KEITO_CAPTURE_DURABLE"));
        assert_eq!(files(&home), before, "state/profile side effect: {args:?}");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "network before refusal: {args:?}"
        );
    }
}
