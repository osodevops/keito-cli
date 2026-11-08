use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn capture_worker_status_never_claims_an_uninstalled_service_running() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("keito")
        .unwrap()
        .env("HOME", home.path())
        .args(["capture", "worker-status", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"running\": false"));
}

#[test]
fn setup_explains_explicit_supervisor_install() {
    Command::cargo_bin("keito")
        .unwrap()
        .args(["capture", "setup", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--install-worker"));
}

#[cfg(unix)]
mod launchd {
    use keito_cli::capture_supervisor::{Supervisor, WorkerConfig};
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::PathBuf;
    fn fixture() -> (tempfile::TempDir, Supervisor, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let launchctl = temp.path().join("launchctl");
        std::fs::write(
            &launchctl,
            r#"#!/bin/sh
state="$(dirname "$0")/running"
fail="$(dirname "$0")/fail"
case "$1" in
  print) if [ -f "$state" ]; then printf 'state = running\npid = 123\n'; exit 0; else exit 1; fi;;
  bootout) rm -f "$state"; exit 0;;
  bootstrap) if [ -f "$fail" ]; then exit 1; fi; touch "$state"; exit 0;;
esac
exit 1
"#,
        )
        .unwrap();
        std::fs::set_permissions(&launchctl, std::fs::Permissions::from_mode(0o700)).unwrap();
        let exe = temp.path().join("keito runtime with spaces");
        std::fs::write(&exe, b"fixture binary").unwrap();
        let root = temp.path().join("project with & spaces");
        std::fs::create_dir(&root).unwrap();
        let capture = temp.path().join("capture");
        let supervisor = Supervisor {
            home: temp.path().join("home"),
            launchctl,
            uid: 123,
        };
        (temp, supervisor, exe, root, capture)
    }
    fn config<'a>(
        exe: &'a std::path::Path,
        root: &'a std::path::Path,
        capture: &'a std::path::Path,
    ) -> WorkerConfig<'a> {
        WorkerConfig {
            repository_key: "repository-fixture",
            repository_root: root,
            executable: exe,
            capture_home: capture,
            workspace_id: "workspace_fixture",
            api_origin: "https://example.invalid",
        }
    }
    #[test]
    fn supervisor_installs_only_owned_service_and_preserves_unrelated_agents() {
        let (temp, s, exe, root, capture) = fixture();
        let conf = config(&exe, &root, &capture);
        let agents = s.home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("unrelated.plist"), b"unrelated settings").unwrap();
        let status = s.install(&conf).unwrap();
        assert!(status.installed);
        assert!(status.running);
        assert!(!status.stale);
        assert!(s.install(&conf).unwrap().running);
        let mut files: Vec<_> = std::fs::read_dir(&agents)
            .unwrap()
            .map(|x| x.unwrap().path())
            .filter(|x| x.file_name().unwrap() != "unrelated.plist")
            .collect();
        assert_eq!(files.len(), 1);
        let owned = files.remove(0);
        let text = std::fs::read_to_string(&owned).unwrap();
        assert!(text.contains("project with &amp; spaces"));
        assert!(!text.contains("KEITO_API_KEY"));
        assert!(text.contains("<key>KeepAlive</key><true/>"));
        assert_eq!(
            std::fs::metadata(&owned).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::write(&exe, b"changed runtime").unwrap();
        assert!(s.status(conf.repository_key, Some(&conf)).unwrap().stale);
        assert!(!s.uninstall(conf.repository_key).unwrap().installed);
        assert_eq!(
            std::fs::read(agents.join("unrelated.plist")).unwrap(),
            b"unrelated settings"
        );
        assert!(!temp.path().join("running").exists());
    }
    #[test]
    fn failed_bootstrap_restores_original_owned_bytes_and_never_claims_running() {
        let (temp, s, exe, root, capture) = fixture();
        let conf = config(&exe, &root, &capture);
        s.install(&conf).unwrap();
        let file = std::fs::read_dir(s.home.join("Library/LaunchAgents"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let prior = std::fs::read(&file).unwrap();
        std::fs::write(&exe, b"updated runtime").unwrap();
        std::fs::write(temp.path().join("fail"), b"fail").unwrap();
        assert!(s.install(&conf).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), prior);
        assert!(!s.status(conf.repository_key, None).unwrap().running);
    }
    #[test]
    fn malicious_origin_and_symlinked_agent_directory_are_refused() {
        let (temp, s, exe, root, capture) = fixture();
        let mut conf = config(&exe, &root, &capture);
        conf.api_origin = "https://secret:password@example.invalid?token=PRIVATE_CANARY";
        let error = s.render(&conf).unwrap_err().to_string();
        assert!(!error.contains("PRIVATE_CANARY"));
        conf.api_origin = "https://example.invalid";
        std::fs::create_dir_all(s.home.join("Library")).unwrap();
        let other = temp.path().join("other");
        std::fs::create_dir(&other).unwrap();
        symlink(&other, s.home.join("Library/LaunchAgents")).unwrap();
        assert!(s.install(&conf).is_err());
        assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0);
    }
    #[test]
    fn symlinked_parent_of_launch_agents_cannot_escape_the_owned_home() {
        let (temp, s, exe, root, capture) = fixture();
        let conf = config(&exe, &root, &capture);
        std::fs::create_dir_all(&s.home).unwrap();
        let outside = temp.path().join("outside-library");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, s.home.join("Library")).unwrap();
        assert!(s.install(&conf).is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }
}
