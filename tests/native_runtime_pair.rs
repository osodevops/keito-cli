//! Native replacement for abandoned Python-pin tests. No installed profile,
//! interpreter, harness, or executable is ever invoked by these fixtures.
#![cfg(unix)]
use keito_cli::{capture, capture_runtime_pair::verify};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
};
struct Pair {
    _temp: tempfile::TempDir,
    core: PathBuf,
    guard: PathBuf,
    manifest: PathBuf,
    hash: String,
}
impl Pair {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let core = root.join("keito");
        let guard = root.join("keito-capture-guard");
        let manifest = root.join("capture-runtime-manifest.json");
        fs::write(&core, b"inert core fixture; must never execute").unwrap();
        fs::write(&guard, b"inert guard fixture; must never execute").unwrap();
        for path in [&core, &guard] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let hash = capture::hash(fs::read(&guard).unwrap());
        fs::write(&manifest, json!({"manifest_contract":1,"version":"0.2.0","target":"test-target","guard_contract":1,"files":{"keito":capture::hash(fs::read(&core).unwrap()),"keito-capture-guard":hash}}).to_string()).unwrap();
        Self {
            _temp: temp,
            core,
            guard,
            manifest,
            hash,
        }
    }
    fn valid(&self) -> bool {
        verify(&self.core, &self.hash, "0.2.0", "test-target").is_ok()
    }
}
#[test]
fn exact_compiled_anchor_and_manifest_qualify_without_execution() {
    let p = Pair::new();
    assert!(p.valid());
}
#[test]
fn unsigned_adjacent_manifest_cannot_replace_compiled_anchor() {
    let p = Pair::new();
    assert!(verify(&p.core, &"0".repeat(64), "0.2.0", "test-target").is_err());
    assert!(verify(&p.core, "", "0.2.0", "test-target").is_err());
}
#[test]
fn changed_or_missing_guard_stays_unqualified() {
    let p = Pair::new();
    fs::write(&p.guard, b"changed").unwrap();
    assert!(!p.valid());
    fs::remove_file(&p.guard).unwrap();
    assert!(!p.valid());
}
#[test]
fn altered_core_or_manifest_is_unqualified() {
    let p = Pair::new();
    fs::write(&p.core, b"changedcore").unwrap();
    assert!(!p.valid());
    let p = Pair::new();
    fs::write(&p.manifest, b"{}").unwrap();
    assert!(!p.valid());
}
#[test]
fn target_version_contract_and_extra_manifest_fields_refuse() {
    let p = Pair::new();
    assert!(verify(&p.core, &p.hash, "0.2.1", "test-target").is_err());
    assert!(verify(&p.core, &p.hash, "0.2.0", "other-target").is_err());
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&p.manifest).unwrap()).unwrap();
    v["untrusted_extra"] = json!(true);
    fs::write(&p.manifest, v.to_string()).unwrap();
    assert!(!p.valid());
    v.as_object_mut().unwrap().remove("untrusted_extra");
    v["guard_contract"] = json!(2);
    fs::write(&p.manifest, v.to_string()).unwrap();
    assert!(!p.valid());
}
#[test]
fn symlinked_or_writable_guard_refuses() {
    let p = Pair::new();
    fs::set_permissions(&p.guard, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(!p.valid());
    let p = Pair::new();
    let replacement = p.guard.with_extension("replacement");
    fs::rename(&p.guard, &replacement).unwrap();
    symlink(&replacement, &p.guard).unwrap();
    assert!(!p.valid());
}
