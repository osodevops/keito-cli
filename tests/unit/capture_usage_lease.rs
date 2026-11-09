//! Real private descriptor/source leases; no harness, CLI or process launch.
use super::*;
use std::{
    ffi::CString,
    fs,
    os::unix::{
        ffi::OsStrExt,
        fs::{symlink, PermissionsExt},
    },
};
const SESSION: &str = "12345678-1234-4234-8234-123456789abc";
struct Fixture {
    _temp: tempfile::TempDir,
    path: PathBuf,
    approved: ApprovedActiveSession,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap().join("home");
        let path = home.join(format!(".claude/projects/owned/{SESSION}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let approved =
            ApprovedActiveSession::new(Harness::ClaudeCode, "2.1.319", &home, &path, SESSION)
                .unwrap();
        Self {
            _temp: temp,
            path,
            approved,
        }
    }
}
#[test]
fn source_identity_is_opaque_and_accepts_same_inode_growth_after_prompt() {
    let f = Fixture::new();
    let (_, baseline) = read_request_ids_leased(&f.approved).unwrap();
    let identity = baseline.identity().unwrap();
    assert_eq!(identity.len(), 64);
    assert!(!identity.contains('/'));
    assert!(baseline.valid());
    fs::write(
        &f.path,
        serde_json::json!({"type":"user","sessionId":SESSION}).to_string() + "\n",
    )
    .unwrap();
    assert!(!baseline.valid());
    let (_, current) = read_requests_leased(&f.approved, &BTreeSet::new(), &identity).unwrap();
    assert_eq!(current.identity(), Some(identity));
    assert!(current.valid());
}
#[test]
fn replaced_source_is_rejected_before_poisoned_bytes_are_decoded_and_new_prompt_can_rebaseline() {
    let f = Fixture::new();
    let (_, baseline) = read_request_ids_leased(&f.approved).unwrap();
    let identity = baseline.identity().unwrap();
    fs::rename(&f.path, f.path.with_extension("previous")).unwrap();
    fs::write(&f.path, b"PRIVATE_POISONED_INVALID_BYTES").unwrap();
    assert!(!baseline.valid());
    assert!(matches!(
        read_requests_leased(&f.approved, &BTreeSet::new(), &identity),
        Err(UsageReadError::SourceChanged)
    ));
    fs::write(&f.path, b"").unwrap();
    let (_, fresh) = read_request_ids_leased(&f.approved).unwrap();
    assert_ne!(fresh.identity(), Some(identity));
    assert!(fresh.valid());
}
#[test]
fn named_symlink_fifo_hardlink_or_directory_never_qualifies_as_source() {
    for kind in [0, 1, 2, 3] {
        let f = Fixture::new();
        let (_, baseline) = read_request_ids_leased(&f.approved).unwrap();
        let old = f.path.with_extension("previous");
        fs::rename(&f.path, &old).unwrap();
        match kind {
            0 => symlink(&old, &f.path).unwrap(),
            1 => {
                let name = CString::new(f.path.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
            2 => fs::hard_link(&old, &f.path).unwrap(),
            _ => fs::create_dir(&f.path).unwrap(),
        }
        assert!(!baseline.valid());
        assert!(read_request_ids_leased(&f.approved).is_err());
    }
}
#[test]
fn same_inode_content_or_permissions_change_invalidates_the_commit_held_lease() {
    for mode in [false, true] {
        let f = Fixture::new();
        let (_, lease) = read_request_ids_leased(&f.approved).unwrap();
        if mode {
            fs::set_permissions(&f.path, fs::Permissions::from_mode(0o400)).unwrap();
        } else {
            fs::write(&f.path, b"changed").unwrap();
        }
        assert!(!lease.valid());
    }
}
