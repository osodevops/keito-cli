//! Marker-only regression: no installer, hook, provider or real profile calls.
#![cfg(unix)]
#[path = "support/mutation_environment.rs"]
mod isolation;
use std::{fs, path::Path};
#[test]
fn missing_or_foreign_mutation_environment_never_reaches_a_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    let capture = home.join("capture");
    fs::create_dir_all(&capture).unwrap();
    let core = root.join("owned-core");
    fs::write(&core, b"developer fixture, never executed").unwrap();
    let marker = capture.join("mutation-must-not-happen");
    let cases = [
        (None, Some(capture.as_path()), Some(core.as_path())),
        (Some(home.as_path()), None, Some(core.as_path())),
        (
            Some(Path::new("/foreign-profile")),
            Some(capture.as_path()),
            Some(core.as_path()),
        ),
        (
            Some(home.as_path()),
            Some(Path::new("/foreign-capture")),
            Some(core.as_path()),
        ),
        (
            Some(home.as_path()),
            Some(capture.as_path()),
            Some(Path::new("/foreign-real-cli")),
        ),
    ];
    for (home, capture, core) in cases {
        let result = isolation::checked(&root, &root, home, capture, core);
        assert!(result.is_err());
        if result.is_ok() {
            fs::write(&marker, b"unsafe").unwrap();
        }
        assert!(!marker.exists());
    }
    let valid = isolation::checked(&root, &root, Some(&home), Some(&capture), Some(&core)).unwrap();
    assert_eq!(valid.home, home);
    assert_eq!(valid.core, core);
    fs::write(valid.capture.join("valid-fixture-marker"), b"isolated").unwrap();
}
#[test]
fn owned_temporary_harness_symlinks_are_refused_before_mutation() {
    for name in [".codex", ".claude"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let capture = home.join("capture");
        fs::create_dir_all(&capture).unwrap();
        let core = root.join("owned-core");
        fs::write(&core, b"never executed").unwrap();
        let other = root.join("other-owned-fixture");
        fs::create_dir(&other).unwrap();
        std::os::unix::fs::symlink(&other, home.join(name)).unwrap();
        assert!(
            isolation::checked(&root, &root, Some(&home), Some(&capture), Some(&core)).is_err()
        );
        assert_eq!(fs::read_dir(other).unwrap().count(), 0);
    }
}
