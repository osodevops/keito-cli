#![allow(deprecated)]

#[cfg(unix)]
use assert_cmd::Command;

#[test]
#[cfg(unix)]
fn bundled_skill_hooks_record_sessions_with_fake_keito() {
    Command::new("bash")
        .arg("tests/skill_hooks.sh")
        .assert()
        .success();
}
