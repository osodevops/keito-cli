use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn capture_worker_status_never_claims_an_uninstalled_service_running() {
    let home=tempfile::tempdir().unwrap();
    Command::cargo_bin("keito").unwrap().env("HOME",home.path())
        .args(["capture","worker-status","--json"]).assert().success()
        .stdout(predicate::str::contains("\"running\": false"));
}

#[test]
fn setup_explains_explicit_supervisor_install() {
    Command::cargo_bin("keito").unwrap().args(["capture","setup","--help"]).assert().success()
        .stdout(predicate::str::contains("--install-worker"));
}
