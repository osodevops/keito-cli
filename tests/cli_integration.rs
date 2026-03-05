#![allow(deprecated)]
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_flag_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Track billable time"));
}

#[test]
fn auth_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["auth", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("login"));
}

#[test]
fn time_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["time", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("start"));
}

#[test]
fn projects_help_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["projects", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"));
}

#[test]
fn whoami_without_auth_fails() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["auth", "whoami"])
        .env_remove("KEITO_API_KEY")
        .env_remove("KEITO_WORKSPACE_ID")
        .assert()
        .failure()
        .code(1);
}

#[test]
fn time_stop_help_shows_discard() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["time", "stop", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--discard"));
}

#[test]
fn version_flag_works() {
    Command::cargo_bin("keito")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("keito"));
}
