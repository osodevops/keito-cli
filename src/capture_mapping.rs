//! Recoverable mapping-hint publication; never grants consent itself.
use crate::adapter_files::{checkpoint, conflict, digest, failure, hex, identifier, Dir};
use crate::error::AppError;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const HINT: &str = "config.yml";
const JOURNAL: &str = ".capture-mapping-recovery.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    schema_version: u8,
    original: Option<String>,
    mode: u32,
    candidate_sha256: String,
}

/// Holds the private setup lock until successful commit or conflict-aware rollback.
pub struct MappingIds<'a> {
    pub workspace: &'a str,
    pub client: &'a str,
    pub project: &'a str,
    pub task: &'a str,
}

pub struct MappingPublication {
    directory: Dir,
    _lock: File,
    original: Option<Vec<u8>>,
    candidate: Vec<u8>,
    finished: bool,
}
impl MappingPublication {
    pub fn begin(root: &Path, ids: &MappingIds) -> Result<Self, AppError> {
        if [ids.workspace, ids.client, ids.project, ids.task]
            .iter()
            .any(|v| !identifier(v, 128))
        {
            return Err(AppError::InvalidInput(
                "Mapping identifiers are invalid; no mapping was replaced.".into(),
            ));
        }
        let directory = Dir::anchor(root)?.child(".keito", true)?;
        let lock = directory.lock(".capture-setup.lock")?;
        if directory.exists(JOURNAL)? {
            restore_pending(&directory)?;
            return Err(AppError::Conflict("Recovered the prior mapping after interrupted setup. Core consent may have committed; inspect capture status and explicitly reapprove before continuing.".into()));
        }
        let original = directory.read(HINT, 64 * 1024)?;
        let mode = if original.is_some() {
            directory
                .open(HINT)?
                .metadata()
                .map_err(|_| failure())?
                .permissions()
                .mode()
                & 0o777
        } else {
            0o600
        };
        let candidate=serde_json::to_vec_pretty(&serde_json::json!({"version":1,"workspace_id":ids.workspace,"client_id":ids.client,"project_id":ids.project,"task_id":ids.task,"agent_tracking":{"enabled":true,"source":"agent","redact_notes":true}})).map_err(|_|failure())?;
        let recovery = Recovery {
            schema_version: 1,
            original: original.as_ref().map(|b| STANDARD.encode(b)),
            mode,
            candidate_sha256: digest(&candidate),
        };
        directory.atomic(
            JOURNAL,
            &serde_json::to_vec(&recovery).map_err(|_| failure())?,
            0o600,
            Some(None),
            "mapping-journal-fsynced",
        )?;
        let guard = Self {
            directory,
            _lock: lock,
            original,
            candidate,
            finished: false,
        };
        guard.directory.atomic(
            HINT,
            &guard.candidate,
            0o600,
            Some(guard.original.as_deref()),
            "mapping-candidate-fsynced",
        )?;
        checkpoint("mapping-published");
        Ok(guard)
    }
    pub fn commit(&mut self) -> Result<(), AppError> {
        if self.finished {
            return Err(failure());
        }
        self.directory.verify()?;
        if self.directory.read(HINT, 64 * 1024)?.as_deref() != Some(self.candidate.as_slice()) {
            return Err(conflict());
        }
        self.directory.unlink(JOURNAL, false)?;
        self.finished = true;
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<(), AppError> {
        if self.finished {
            return Ok(());
        }
        restore_pending(&self.directory)?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for MappingPublication {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback();
        }
    }
}
fn restore_pending(directory: &Dir) -> Result<(), AppError> {
    directory.verify()?;
    let bytes = directory.read(JOURNAL, 256 * 1024)?.ok_or_else(failure)?;
    let recovery: Recovery = serde_json::from_slice(&bytes).map_err(|_| failure())?;
    if recovery.schema_version != 1 || recovery.mode > 0o777 || !hex(&recovery.candidate_sha256, 64)
    {
        return Err(failure());
    }
    let original = recovery
        .original
        .map(|b| STANDARD.decode(b).map_err(|_| failure()))
        .transpose()?;
    if original.as_ref().is_some_and(|b| b.len() > 64 * 1024) {
        return Err(failure());
    }
    let current = directory.read(HINT, 64 * 1024)?;
    if current != original {
        if !current
            .as_ref()
            .is_some_and(|b| digest(b) == recovery.candidate_sha256)
        {
            return Err(conflict());
        }
        match original {
            Some(ref original) => directory.atomic(
                HINT,
                original,
                recovery.mode,
                Some(current.as_deref()),
                "mapping-rollback-fsynced",
            )?,
            None => {
                checkpoint("mapping-delete-before");
                if directory.read(HINT, 64 * 1024)? != current {
                    return Err(conflict());
                }
                directory.unlink(HINT, false)?;
            }
        }
    }
    directory.unlink(JOURNAL, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter_files::set_hook;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    fn begin(root: &Path) -> Result<MappingPublication, AppError> {
        MappingPublication::begin(
            root,
            &MappingIds {
                workspace: "w1",
                client: "c1",
                project: "p1",
                task: "t1",
            },
        )
    }
    #[test]
    fn failed_core_restores_exact_disabled_hint_and_mode() {
        let r = tempfile::tempdir().unwrap();
        let d = r.path().join(".keito");
        fs::create_dir(&d).unwrap();
        let old = b"{\"agent_tracking\":{\"enabled\":false},\"project_id\":\"prior\"}\n";
        fs::write(d.join(HINT), old).unwrap();
        fs::set_permissions(d.join(HINT), fs::Permissions::from_mode(0o640)).unwrap();
        let guard = begin(r.path()).unwrap();
        assert_ne!(fs::read(d.join(HINT)).unwrap(), old);
        drop(guard);
        assert_eq!(fs::read(d.join(HINT)).unwrap(), old);
        assert_eq!(
            fs::metadata(d.join(HINT)).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(!d.join(JOURNAL).exists());
    }
    #[test]
    fn successful_setup_commits_hint_without_recovery_data() {
        let r = tempfile::tempdir().unwrap();
        begin(r.path()).unwrap().commit().unwrap();
        let d = r.path().join(".keito");
        let v: serde_json::Value =
            serde_json::from_slice(&fs::read(d.join(HINT)).unwrap()).unwrap();
        assert_eq!(v["client_id"], "c1");
        assert_eq!(v["agent_tracking"]["enabled"], true);
        assert_eq!(
            fs::metadata(d.join(HINT)).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!d.join(JOURNAL).exists());
    }
    #[test]
    fn failed_new_setup_removes_only_its_candidate() {
        let r = tempfile::tempdir().unwrap();
        begin(r.path()).unwrap().rollback().unwrap();
        assert!(!r.path().join(".keito/config.yml").exists());
    }
    #[test]
    fn lock_serializes_consent_publication() {
        let r = tempfile::tempdir().unwrap();
        let guard = begin(r.path()).unwrap();
        assert!(begin(r.path()).is_err());
        drop(guard);
        assert!(begin(r.path()).is_ok());
    }
    #[test]
    fn external_edit_is_retained_with_journal_and_blocks_replay() {
        let r = tempfile::tempdir().unwrap();
        let mut guard = begin(r.path()).unwrap();
        let d = r.path().join(".keito");
        fs::write(d.join(HINT), b"external disabled hint").unwrap();
        assert!(guard.rollback().is_err());
        assert_eq!(fs::read(d.join(HINT)).unwrap(), b"external disabled hint");
        assert!(d.join(JOURNAL).exists());
        assert!(begin(r.path()).is_err());
        assert_eq!(fs::read(d.join(HINT)).unwrap(), b"external disabled hint");
    }
    #[test]
    fn external_edit_after_candidate_fsync_is_never_replaced() {
        let r = tempfile::tempdir().unwrap();
        let d = r.path().join(".keito");
        let target = d.join(HINT);
        let edited = target.clone();
        set_hook(Some(Box::new(move |p| {
            if p == "mapping-candidate-fsynced" {
                fs::write(&edited, b"late external").unwrap();
            }
        })));
        let result = begin(r.path());
        set_hook(None);
        assert!(result.is_err());
        assert_eq!(fs::read(target).unwrap(), b"late external");
        assert!(d.join(JOURNAL).exists());
    }
    #[test]
    fn external_edit_after_rollback_fsync_is_never_replaced() {
        let r = tempfile::tempdir().unwrap();
        let d = r.path().join(".keito");
        fs::create_dir(&d).unwrap();
        fs::write(d.join(HINT), b"old disabled").unwrap();
        let mut guard = begin(r.path()).unwrap();
        let edited = d.join(HINT);
        set_hook(Some(Box::new(move |p| {
            if p == "mapping-rollback-fsynced" {
                fs::write(&edited, b"late rollback edit").unwrap();
            }
        })));
        let result = guard.rollback();
        set_hook(None);
        assert!(result.is_err());
        assert_eq!(fs::read(d.join(HINT)).unwrap(), b"late rollback edit");
        assert!(d.join(JOURNAL).exists());
    }
    #[test]
    fn abandoned_publication_recovers_then_stops_before_reapproval() {
        let r = tempfile::tempdir().unwrap();
        let mut guard = begin(r.path()).unwrap();
        let d = r.path().join(".keito");
        guard.finished = true;
        drop(guard);
        assert!(begin(r.path()).is_err());
        assert!(!d.join(HINT).exists());
        assert!(!d.join(JOURNAL).exists());
    }
    #[test]
    fn invalid_ids_and_symlinks_never_modify_external_files() {
        let r = tempfile::tempdir().unwrap();
        assert!(MappingPublication::begin(
            r.path(),
            &MappingIds {
                workspace: "../bad",
                client: "c",
                project: "p",
                task: "t"
            }
        )
        .is_err());
        assert!(!r.path().join(".keito").exists());
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join(HINT), b"outside").unwrap();
        symlink(external.path(), r.path().join(".keito")).unwrap();
        assert!(begin(r.path()).is_err());
        assert_eq!(fs::read(external.path().join(HINT)).unwrap(), b"outside");
    }
    #[test]
    fn corrupt_recovery_never_echoes_content_or_follows_paths() {
        let r = tempfile::tempdir().unwrap();
        let d = r.path().join(".keito");
        fs::create_dir(&d).unwrap();
        fs::write(d.join(HINT), b"preserve").unwrap();
        for bad in [
            b"[]".as_slice(),
            b"{\"schema_version\":1,\"original\":\"PRIVATE_SECRET\"}",
        ] {
            fs::write(d.join(JOURNAL), bad).unwrap();
            let e = match begin(r.path()) {
                Ok(_) => panic!("unsafe recovery"),
                Err(e) => e.to_string(),
            };
            assert!(!e.contains("PRIVATE_SECRET"));
            assert!(!e.contains(&r.path().display().to_string()));
            assert_eq!(fs::read(d.join(HINT)).unwrap(), b"preserve");
        }
    }
    #[test]
    fn legacy_python_base64_journal_recovers_exact_prior_bytes_and_stops() {
        let r = tempfile::tempdir().unwrap();
        let d = r.path().join(".keito");
        fs::create_dir(&d).unwrap();
        let old = b"{\"agent_tracking\":{\"enabled\":false}}\n";
        let candidate = b"{\"project_id\":\"candidate\"}";
        fs::write(d.join(HINT), candidate).unwrap();
        let journal = serde_json::json!({"schema_version":1,"original":STANDARD.encode(old),"mode":416,"candidate_sha256":digest(candidate)});
        fs::write(d.join(JOURNAL), serde_json::to_vec(&journal).unwrap()).unwrap();
        assert!(begin(r.path()).is_err());
        assert_eq!(fs::read(d.join(HINT)).unwrap(), old);
        assert_eq!(
            fs::metadata(d.join(HINT)).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(!d.join(JOURNAL).exists());
    }
    #[test]
    fn mapping_signal_fixture_child() {
        let Ok(root) = std::env::var("KEITO_TEST_MAPPING_KILL_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root).canonicalize().unwrap();
        let home = root.join("home with ' literal $");
        assert_eq!(
            std::env::var_os("HOME").map(std::path::PathBuf::from),
            Some(home.clone())
        );
        assert_eq!(
            std::env::var_os("KEITO_CAPTURE_HOME").map(std::path::PathBuf::from),
            Some(home.join("capture"))
        );
        set_hook(Some(Box::new(move |p| {
            if p == "mapping-published" {
                unsafe {
                    libc::kill(libc::getpid(), libc::SIGKILL);
                }
            }
        })));
        let _guard = begin(&root.join("repository")).unwrap();
        panic!("SIGKILL boundary was not reached");
    }
    #[test]
    fn actual_sigkill_restores_hint_on_next_begin_without_replaying_consent() {
        use std::os::unix::process::ExitStatusExt;
        let r = tempfile::tempdir().unwrap();
        let home = r.path().join("home with ' literal $");
        let capture = home.join("capture");
        fs::create_dir_all(&capture).unwrap();
        let repo = r.path().join("repository");
        let d = repo.join(".keito");
        fs::create_dir_all(&d).unwrap();
        let old = b"{\"agent_tracking\":{\"enabled\":false},\"project_id\":\"prior\"}";
        fs::write(d.join(HINT), old).unwrap();
        let output = crate::skill_adapter::tests::isolated_fixture_process(
            r.path(),
            Some(&home),
            Some(&capture),
        )
        .unwrap()
        .args([
            "--exact",
            "capture_mapping::tests::mapping_signal_fixture_child",
            "--nocapture",
        ])
        .env(
            "KEITO_TEST_MAPPING_KILL_ROOT",
            r.path().canonicalize().unwrap(),
        )
        .output()
        .unwrap();
        assert_eq!(output.status.signal(), Some(libc::SIGKILL));
        assert_ne!(fs::read(d.join(HINT)).unwrap(), old);
        assert!(d.join(JOURNAL).exists());
        assert!(begin(&repo).is_err());
        assert_eq!(fs::read(d.join(HINT)).unwrap(), old);
        assert!(!d.join(JOURNAL).exists());
    }
}
