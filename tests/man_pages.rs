#![allow(deprecated)]

use assert_cmd::Command;
use std::collections::BTreeSet;
use std::fs;

const EXPECTED_PAGES: &[&str] = &[
    "keito.1",
    "keito-help.1",
    "keito-auth.1",
    "keito-auth-help.1",
    "keito-auth-login.1",
    "keito-auth-logout.1",
    "keito-auth-status.1",
    "keito-auth-whoami.1",
    "keito-projects.1",
    "keito-projects-help.1",
    "keito-projects-list.1",
    "keito-projects-show.1",
    "keito-projects-tasks.1",
    "keito-time.1",
    "keito-time-help.1",
    "keito-time-start.1",
    "keito-time-stop.1",
    "keito-time-log.1",
    "keito-time-list.1",
    "keito-time-running.1",
];

#[test]
fn gen_man_emits_agent_command_pages() {
    let temp_dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("gen-man")
        .unwrap()
        .arg(temp_dir.path())
        .assert()
        .success();

    let mut generated_pages = BTreeSet::new();
    let mut combined = String::new();

    for entry in fs::read_dir(temp_dir.path()).unwrap() {
        let entry = entry.unwrap();
        let file_name = entry.file_name().to_string_lossy().to_string();
        generated_pages.insert(file_name.clone());
        let contents = fs::read_to_string(entry.path()).unwrap();
        assert!(
            contents.len() > 100,
            "{file_name} should contain real man-page content"
        );
        combined.push_str(&contents);
        combined.push('\n');
    }

    for page in EXPECTED_PAGES {
        assert!(
            generated_pages.contains(*page),
            "expected generated man page {page}"
        );
    }

    assert!(!combined.contains("/api/v2/me"));
    assert!(!combined.contains("app.keito.io"));
    assert!(!combined.contains("is_billable"));

    let normalized = combined.replace("\\-", "-");
    for token in normalized.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '.') {
        if let Some(reference) = token.strip_suffix("(1)") {
            if reference.starts_with("keito") {
                let page = format!("{reference}.1");
                assert!(
                    generated_pages.contains(&page),
                    "man page reference {reference}(1) should have generated page {page}"
                );
            }
        }
    }
}
