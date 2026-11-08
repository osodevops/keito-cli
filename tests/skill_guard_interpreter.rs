#![cfg(unix)]
use keito_cli::capture;
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    image: PathBuf,
    receipt: PathBuf,
    config: PathBuf,
    sentinel: PathBuf,
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
impl Fixture {
    fn new(pinned: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap().join("home with 'quote");
        fs::create_dir_all(&home).unwrap();
        let image = home.join("native interpreter");
        let sentinel = home.join("MUST_NOT_EXECUTE");
        let bytes = format!(
            "#!/bin/sh\nprintf executed >{}\n",
            quote(sentinel.to_str().unwrap())
        );
        fs::write(&image, &bytes).unwrap();
        fs::set_permissions(&image, fs::Permissions::from_mode(0o700)).unwrap();
        let generation = home.join(format!(
            ".codex/.keito-capture-generations/{}",
            "a".repeat(64)
        ));
        fs::create_dir_all(generation.join("hooks")).unwrap();
        let dispatch = generation.join("hooks/dispatch.sh");
        fs::write(&dispatch, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&dispatch, fs::Permissions::from_mode(0o700)).unwrap();
        let receipt = generation.join("capture-install.json");
        let mut value = json!({"adapter_contract":1,"agent":"codex","harness_version":"0.167.0","files":{"hooks/dispatch.sh":capture::hash("#!/bin/sh\nexit 0\n")},"generation":"a".repeat(64)});
        if pinned {
            value["guard_interpreter"] = json!({"path":image,"sha256":capture::hash(bytes)});
        }
        fs::write(&receipt, value.to_string()).unwrap();
        fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600)).unwrap();
        let skill = home.join(".codex/skills/keito-time-track");
        fs::create_dir_all(&skill).unwrap();
        fs::copy(&receipt, skill.join("capture-install.json")).unwrap();
        let mut hooks = serde_json::Map::new();
        for (event, lifecycle) in [
            ("SessionStart", "session-start"),
            ("UserPromptSubmit", "prompt"),
            ("Stop", "stop"),
            ("SessionEnd", "session-end"),
        ] {
            let pin = if pinned {
                format!("KEITO_CAPTURE_PYTHON={} ", quote(image.to_str().unwrap()))
            } else {
                String::new()
            };
            let command=format!("KEITO_CLI_BIN=keito {pin}{} --agent codex --event {lifecycle} --harness-version 0.167.0",quote(dispatch.to_str().unwrap()));
            hooks.insert(
                event.into(),
                json!([{"hooks":[{"type":"command","command":command,"timeout":1}]}]),
            );
        }
        let config = home.join(".codex/hooks.json");
        fs::write(&config, json!({"hooks":hooks}).to_string()).unwrap();
        Self {
            _temp: temp,
            home,
            image,
            receipt,
            config,
            sentinel,
        }
    }
    fn doctor(&self) -> Value {
        let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("keito"))
            .env("HOME", &self.home)
            .env("KEITO_CAPTURE_HOME", self.home.join("capture"))
            .env_remove("KEITO_API_KEY")
            .env_remove("KEITO_ACCOUNT_ID")
            .args(["skill", "doctor", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !self.sentinel.exists(),
            "doctor must never execute the configured interpreter"
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
#[test]
fn doctor_reports_literal_receipt_bound_image_without_claiming_trust_or_executing_it() {
    let f = Fixture::new(true);
    let status = f.doctor();
    assert_eq!(status["codex"]["guard_interpreter"], "PINNED_UNVERIFIED");
    assert_eq!(status["codex"]["hooks_configured"], true);
    assert_eq!(status["ready"], false);
    assert_eq!(status["hook_trust"], "UNKNOWN_REVIEW_IN_HARNESS");
}
#[test]
fn legacy_ownership_without_a_pin_is_unconfigured_for_the_new_guard_runtime() {
    let f = Fixture::new(false);
    let status = f.doctor();
    assert_eq!(status["codex"]["guard_interpreter"], "UNCONFIGURED");
    assert_eq!(status["codex"]["hooks_configured"], false);
}
#[test]
fn changed_or_missing_interpreter_is_stale_while_the_receipt_remains_readable() {
    let f = Fixture::new(true);
    fs::write(&f.image, "changed image").unwrap();
    let status = f.doctor();
    assert_eq!(status["codex"]["guard_interpreter"], "STALE");
    assert_eq!(status["codex"]["hooks_configured"], false);
    fs::remove_file(&f.image).unwrap();
    assert_eq!(f.doctor()["codex"]["guard_interpreter"], "STALE");
    assert!(f.receipt.exists());
}
#[test]
fn interpreter_pin_must_be_a_literal_assignment_matching_the_registered_receipt() {
    let f = Fixture::new(true);
    let mut value: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
    for event in ["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"] {
        let command = value["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_owned();
        value["hooks"][event][0]["hooks"][0]["command"] =
            json!(format!("echo {}", quote(&command)));
    }
    fs::write(&f.config, value.to_string()).unwrap();
    assert_eq!(f.doctor()["codex"]["hooks_configured"], false);
}
#[test]
fn substituted_or_mismatched_pin_never_qualifies_as_configured() {
    let f = Fixture::new(true);
    let text = fs::read_to_string(&f.config).unwrap();
    fs::write(
        &f.config,
        text.replace("KEITO_CAPTURE_PYTHON=", "KEITO_CAPTURE_PYTHON=$(false)"),
    )
    .unwrap();
    assert_eq!(f.doctor()["codex"]["hooks_configured"], false);
    let mut receipt: Value = serde_json::from_slice(&fs::read(&f.receipt).unwrap()).unwrap();
    receipt["guard_interpreter"]["sha256"] = json!("0".repeat(64));
    fs::write(&f.receipt, receipt.to_string()).unwrap();
    assert_eq!(f.doctor()["codex"]["hooks_configured"], false);
}
