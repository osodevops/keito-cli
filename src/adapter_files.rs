//! Descriptor-relative, no-follow filesystem operations for adapter publication.
//! Errors intentionally omit local paths and file contents.
#![cfg(unix)]
use crate::error::AppError;
use sha2::{Digest, Sha256};
use std::ffi::{CStr, CString, OsString};
use std::fs::{File, Metadata};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) fn failure() -> AppError {
    AppError::Config("Local adapter files need review; inspect capture status and retained recovery metadata before retrying.".into())
}
pub(crate) fn conflict() -> AppError {
    AppError::Conflict("An external edit was preserved. Inspect capture status and retained recovery metadata before retrying.".into())
}
pub(crate) fn digest(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(crate) fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(crate) fn identifier(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
        && !value.contains("..")
}
fn name(value: &str) -> Result<CString, AppError> {
    if value.is_empty() || value == "." || value == ".." || value.contains('/') {
        return Err(failure());
    }
    CString::new(value).map_err(|_| failure())
}
pub(crate) fn nonce() -> Result<String, AppError> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| failure())?;
    Ok(digest(bytes))
}
#[derive(Clone)]
pub(crate) struct Dir {
    file: Arc<File>,
    pub(crate) path: PathBuf,
    parent: Option<(Box<Dir>, String)>,
}
impl Dir {
    pub(crate) fn anchor(path: &Path) -> Result<Self, AppError> {
        let path = path.canonicalize().map_err(|_| failure())?;
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| failure())?;
        // SAFETY: c is NUL terminated; returned descriptor is uniquely owned.
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(failure());
        }
        let result = Self {
            file: Arc::new(unsafe { File::from_raw_fd(fd) }),
            path,
            parent: None,
        };
        result.check_owner()?;
        Ok(result)
    }
    fn check_owner(&self) -> Result<(), AppError> {
        let m = self.file.metadata().map_err(|_| failure())?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } {
            return Err(failure());
        }
        Ok(())
    }
    pub(crate) fn verify(&self) -> Result<(), AppError> {
        self.check_owner()?;
        if let Some((parent, child)) = &self.parent {
            parent.verify()?;
            let expected = self.file.metadata().map_err(|_| failure())?;
            let actual = parent
                .open_directory(child)?
                .file
                .metadata()
                .map_err(|_| failure())?;
            if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
                return Err(conflict());
            }
        } else {
            let m = std::fs::symlink_metadata(&self.path).map_err(|_| failure())?;
            let own = self.file.metadata().map_err(|_| failure())?;
            if !m.is_dir() || m.dev() != own.dev() || m.ino() != own.ino() {
                return Err(conflict());
            }
        }
        Ok(())
    }
    fn open_directory(&self, child: &str) -> Result<Self, AppError> {
        let c = name(child)?;
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(failure());
        }
        let dir = Self {
            file: Arc::new(unsafe { File::from_raw_fd(fd) }),
            path: self.path.join(child),
            parent: Some((Box::new(self.clone()), child.into())),
        };
        dir.check_owner()?;
        Ok(dir)
    }
    pub(crate) fn child(&self, child: &str, create: bool) -> Result<Self, AppError> {
        self.verify()?;
        let c = name(child)?;
        if create
            && unsafe { libc::mkdirat(self.file.as_raw_fd(), c.as_ptr(), 0o700) } != 0
            && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(failure());
        }
        let result = self.open_directory(child)?;
        if create {
            self.sync()?;
        }
        Ok(result)
    }
    pub(crate) fn exists(&self, child: &str) -> Result<bool, AppError> {
        let c = name(child)?;
        let mut m = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                c.as_ptr(),
                m.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            return Ok(true);
        }
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(failure())
        }
    }
    pub(crate) fn open(&self, child: &str) -> Result<File, AppError> {
        let c = name(child)?;
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(failure());
        }
        let f = unsafe { File::from_raw_fd(fd) };
        let m = f.metadata().map_err(|_| failure())?;
        if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.nlink() != 1 {
            return Err(failure());
        }
        Ok(f)
    }
    pub(crate) fn read(&self, child: &str, max: usize) -> Result<Option<Vec<u8>>, AppError> {
        if !self.exists(child)? {
            return Ok(None);
        }
        let f = self.open(child)?;
        if f.metadata().map_err(|_| failure())?.len() > max as u64 {
            return Err(failure());
        }
        let mut bytes = Vec::new();
        f.take(max as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| failure())?;
        if bytes.len() > max {
            return Err(failure());
        }
        Ok(Some(bytes))
    }
    pub(crate) fn lock(&self, child: &str) -> Result<File, AppError> {
        self.verify()?;
        let c = name(child)?;
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c.as_ptr(),
                libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(failure());
        }
        let f = unsafe { File::from_raw_fd(fd) };
        let m = f.metadata().map_err(|_| failure())?;
        if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.nlink() != 1 {
            return Err(failure());
        }
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(AppError::Conflict(
                "Another local adapter operation is active. Wait, then inspect capture status."
                    .into(),
            ));
        }
        Ok(f)
    }
    pub(crate) fn sync(&self) -> Result<(), AppError> {
        installer_phase_event(20);
        let result = self.file.sync_all();
        installer_phase_event(if result.is_ok() { 21 } else { 22 });
        result.map_err(|_| failure())?;
        checkpoint("adapter-directory-fsynced");
        Ok(())
    }
    pub(crate) fn atomic(
        &self,
        child: &str,
        bytes: &[u8],
        mode: u32,
        expected: Option<Option<&[u8]>>,
        phase: &str,
    ) -> Result<(), AppError> {
        self.atomic_checked(child, bytes, mode, expected, phase, &mut || Ok(()))
    }
    pub(crate) fn atomic_checked(
        &self,
        child: &str,
        bytes: &[u8],
        mode: u32,
        expected: Option<Option<&[u8]>>,
        phase: &str,
        check: &mut impl FnMut() -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        match phase {
            "install-payload-file-fsynced" => installer_phase_event(50),
            "install-journal-fsynced" => installer_phase_event(52),
            "install-config-fsynced" => installer_phase_event(53),
            _ => {}
        }
        self.verify()?;
        let temp = format!(".keito-publish-{}", nonce()?);
        let c = name(&temp)?;
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode,
            )
        };
        if fd < 0 {
            return Err(failure());
        }
        let result = (|| {
            let mut f = unsafe { File::from_raw_fd(fd) };
            f.set_permissions(std::fs::Permissions::from_mode(mode))
                .map_err(|_| failure())?;
            f.write_all(bytes)
                .and_then(|_| {
                    installer_phase_event(10);
                    let result = f.sync_all();
                    installer_phase_event(if result.is_ok() { 11 } else { 12 });
                    result
                })
                .map_err(|_| failure())?;
            checkpoint(phase);
            publication_fault(phase)?;
            self.verify()?;
            if let Some(expected) = expected {
                if self.read(child, 32 * 1024 * 1024)?.as_deref() != expected {
                    return Err(conflict());
                }
            }
            check()?;
            // rename flushes every affected directory before returning. No
            // namespace change here requires another flush of that same state.
            self.rename(&temp, self, child)
        })();
        if self.exists(&temp).unwrap_or(false) {
            let _ = self.unlink(&temp, false);
        }
        result
    }
    pub(crate) fn rename(&self, child: &str, dest: &Self, target: &str) -> Result<(), AppError> {
        self.verify()?;
        dest.verify()?;
        let a = name(child)?;
        let b = name(target)?;
        if unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                a.as_ptr(),
                dest.file.as_raw_fd(),
                b.as_ptr(),
            )
        } != 0
        {
            return Err(failure());
        }
        checkpoint("adapter-entry-renamed");
        self.sync()?;
        // Atomic replacement passes the identical anchored directory handle.
        // Distinct handles conservatively keep both flushes, even for aliases.
        if !Arc::ptr_eq(&self.file, &dest.file) {
            dest.sync()?;
        }
        Ok(())
    }
    pub(crate) fn unlink(&self, child: &str, directory: bool) -> Result<(), AppError> {
        self.verify()?;
        let c = name(child)?;
        if unsafe {
            libc::unlinkat(
                self.file.as_raw_fd(),
                c.as_ptr(),
                if directory { libc::AT_REMOVEDIR } else { 0 },
            )
        } != 0
        {
            return Err(failure());
        }
        self.sync()
    }
    pub(crate) fn entries(&self) -> Result<Vec<String>, AppError> {
        // openat "." gives an independent directory cursor; dup would share it.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(failure());
        }
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            unsafe { libc::close(fd) };
            return Err(failure());
        }
        let mut out = Vec::new();
        let result = loop {
            set_errno(0);
            let item = unsafe { libc::readdir(stream) };
            if item.is_null() {
                break if errno() == 0 { Ok(()) } else { Err(failure()) };
            }
            let raw = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) }.to_bytes();
            if raw == b"." || raw == b".." {
                continue;
            }
            if out.len() >= 4096 {
                break Err(failure());
            }
            match OsString::from_vec(raw.to_vec()).into_string() {
                Ok(s) => out.push(s),
                Err(_) => break Err(failure()),
            }
        };
        unsafe { libc::closedir(stream) };
        result?;
        out.sort();
        Ok(out)
    }
    pub(crate) fn remove_tree(&self, child: &str) -> Result<(), AppError> {
        self.remove_tree_checked(child, &mut || Ok(()))
    }
    pub(crate) fn remove_tree_checked(
        &self,
        child: &str,
        check: &mut impl FnMut() -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        check()?;
        let dir = self.child(child, false)?;
        check()?;
        for entry in dir.entries()? {
            check()?;
            if dir.child(&entry, false).is_ok() {
                dir.remove_tree_checked(&entry, check)?;
            } else {
                check()?;
                let _ = dir.open(&entry)?;
                check()?;
                dir.unlink(&entry, false)?;
                check()?;
            }
        }
        check()?;
        self.unlink(child, true)?;
        check()
    }
    pub(crate) fn metadata(&self) -> Result<Metadata, AppError> {
        self.file.metadata().map_err(|_| failure())
    }
}
#[cfg(target_os = "macos")]
fn set_errno(value: i32) {
    unsafe {
        *libc::__error() = value;
    }
}
#[cfg(target_os = "macos")]
fn errno() -> i32 {
    unsafe { *libc::__error() }
}
#[cfg(not(target_os = "macos"))]
fn set_errno(value: i32) {
    unsafe {
        *libc::__errno_location() = value;
    }
}
#[cfg(not(target_os = "macos"))]
fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}
#[cfg(not(test))]
pub(crate) fn checkpoint(_: &str) {}
#[cfg(test)]
type PhaseHook = Box<dyn Fn(&str)>;
#[cfg(test)]
thread_local! {static HOOK: std::cell::RefCell<Option<PhaseHook>> = std::cell::RefCell::new(None);}
#[cfg(test)]
pub(crate) fn checkpoint(phase: &str) {
    HOOK.with(|h| {
        if let Some(f) = h.borrow().as_ref() {
            f(phase);
        }
    })
}
#[cfg(test)]
pub(crate) fn set_hook(hook: Option<PhaseHook>) {
    HOOK.with(|h| *h.borrow_mut() = hook);
}

#[cfg(not(test))]
fn publication_fault(_: &str) -> Result<(), AppError> {
    Ok(())
}
#[cfg(test)]
thread_local! {static FAILURE_PHASE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };}
#[cfg(test)]
fn publication_fault(phase: &str) -> Result<(), AppError> {
    FAILURE_PHASE.with(|p| {
        if p.borrow().as_deref() == Some(phase) {
            Err(failure())
        } else {
            Ok(())
        }
    })
}
#[cfg(test)]
pub(crate) fn set_publication_fault(phase: Option<&str>) {
    FAILURE_PHASE.with(|p| *p.borrow_mut() = phase.map(str::to_owned));
}

#[cfg(test)]
mod installer_flush_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    struct Observe(Rc<RefCell<Vec<String>>>);
    impl Observe {
        fn start() -> Self {
            let events = Rc::new(RefCell::new(Vec::new()));
            let copy = Rc::clone(&events);
            set_hook(Some(Box::new(move |phase| {
                copy.borrow_mut().push(phase.to_owned());
            })));
            Self(events)
        }
        fn events(&self) -> Vec<String> {
            self.0.borrow().clone()
        }
    }
    impl Drop for Observe {
        fn drop(&mut self) {
            set_hook(None);
        }
    }

    #[test]
    fn recovery_owner_already_refused_preserves_the_entire_tree() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::anchor(root.path()).unwrap();
        let target = dir.child("stage", true).unwrap();
        std::fs::write(target.path.join("a"), b"a").unwrap();
        let result = dir.remove_tree_checked("stage", &mut || {
            Err(AppError::Conflict("Original recovery owner refusal".into()))
        });
        assert!(
            matches!(result, Err(AppError::Conflict(message)) if message == "Original recovery owner refusal")
        );
        assert_eq!(target.read("a", 16).unwrap().unwrap(), b"a");
        assert!(root.path().join("stage").is_dir());
    }
    #[test]
    fn recovery_owner_refusal_after_first_deletion_preserves_remaining_files() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::anchor(root.path()).unwrap();
        let target = dir.child("stage", true).unwrap();
        std::fs::write(target.path.join("a"), b"a").unwrap();
        std::fs::write(target.path.join("b"), b"b").unwrap();
        let result = dir.remove_tree_checked("stage", &mut || {
            if target.path.join("a").exists() {
                Ok(())
            } else {
                Err(AppError::Conflict("Original recovery owner refusal".into()))
            }
        });
        assert!(
            matches!(result, Err(AppError::Conflict(message)) if message == "Original recovery owner refusal")
        );
        assert!(!target.path.join("a").exists());
        assert_eq!(target.read("b", 16).unwrap().unwrap(), b"b");
        assert!(root.path().join("stage").is_dir());
    }
    #[test]
    fn installer_flush_atomic_syncs_file_then_renamed_directory_once() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::anchor(root.path()).unwrap();
        let observe = Observe::start();
        dir.atomic("payload", b"new", 0o600, Some(None), "payload-file-fsynced")
            .unwrap();
        assert_eq!(
            observe.events(),
            [
                "payload-file-fsynced",
                "adapter-entry-renamed",
                "adapter-directory-fsynced"
            ]
        );
        assert_eq!(dir.read("payload", 16).unwrap().unwrap(), b"new");
    }

    #[test]
    fn installer_flush_cross_directory_rename_flushes_both_parents() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::anchor(root.path()).unwrap();
        let destination = dir.child("destination", true).unwrap();
        std::fs::write(root.path().join("payload"), b"new").unwrap();
        let observe = Observe::start();
        dir.rename("payload", &destination, "payload").unwrap();
        assert_eq!(
            observe.events(),
            [
                "adapter-entry-renamed",
                "adapter-directory-fsynced",
                "adapter-directory-fsynced"
            ]
        );
        assert!(!dir.exists("payload").unwrap());
        assert_eq!(destination.read("payload", 16).unwrap().unwrap(), b"new");
    }

    #[test]
    fn installer_flush_distinct_handles_remain_conservative() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::anchor(root.path()).unwrap();
        let second = Dir::anchor(root.path()).unwrap();
        std::fs::write(root.path().join("old"), b"new").unwrap();
        let observe = Observe::start();
        dir.rename("old", &second, "new").unwrap();
        assert_eq!(
            observe.events(),
            [
                "adapter-entry-renamed",
                "adapter-directory-fsynced",
                "adapter-directory-fsynced"
            ]
        );
    }

    #[test]
    fn installer_flush_guard_refusal_never_switches_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::anchor(root.path()).unwrap();
        std::fs::write(root.path().join("payload"), b"old").unwrap();
        let observe = Observe::start();
        assert!(dir
            .atomic_checked(
                "payload",
                b"new",
                0o600,
                Some(Some(b"old")),
                "payload-file-fsynced",
                &mut || Err(failure())
            )
            .is_err());
        assert_eq!(
            observe.events(),
            ["payload-file-fsynced", "adapter-directory-fsynced"]
        );
        assert_eq!(dir.read("payload", 16).unwrap().unwrap(), b"old");
        assert_eq!(dir.entries().unwrap(), ["payload"]);
    }

    #[test]
    fn installer_flush_cas_refusal_preserves_foreign_bytes() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::anchor(root.path()).unwrap();
        std::fs::write(root.path().join("payload"), b"foreign").unwrap();
        let observe = Observe::start();
        assert!(dir
            .atomic(
                "payload",
                b"new",
                0o600,
                Some(Some(b"old")),
                "payload-file-fsynced"
            )
            .is_err());
        assert_eq!(
            observe.events(),
            ["payload-file-fsynced", "adapter-directory-fsynced"]
        );
        assert_eq!(dir.read("payload", 16).unwrap().unwrap(), b"foreign");
        assert_eq!(dir.entries().unwrap(), ["payload"]);
    }
}

// Compiled out of default releases. No path/content is accepted by this API.
#[cfg(not(feature = "dev-tools"))]
#[inline(always)]
pub(crate) fn installer_phase_event(_: u8) {}
#[cfg(feature = "dev-tools")]
pub(crate) fn installer_phase_event(code: u8) {
    installer_phase_trace::emit(code);
}
#[cfg(feature = "dev-tools")]
pub(crate) mod installer_phase_trace {
    use std::{cell::RefCell, io};
    pub(super) const MAX_FRAMES: u32 = 2048;
    struct State {
        fd: i32,
        sequence: u32,
        failed: bool,
    }
    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }
    pub(crate) struct Guard {
        fd: i32,
        flags: i32,
        enabled: bool,
        finished: bool,
    }
    pub(crate) fn with_trace<T>(
        initialize: impl FnOnce() -> io::Result<Guard>,
        run: impl FnOnce() -> Result<T, crate::error::AppError>,
    ) -> Result<T, crate::error::AppError> {
        // Observation setup cannot decide whether the actual installer runs.
        let mut guard = initialize().ok();
        let result = run();
        if let Some(guard) = guard.as_mut() {
            guard.finish(result.is_ok());
        }
        result
    }
    impl Guard {
        pub(crate) fn begin(enabled: bool) -> io::Result<Self> {
            if !enabled {
                return Ok(Self {
                    fd: -1,
                    flags: 0,
                    enabled: false,
                    finished: false,
                });
            }
            Self::begin_fd(libc::STDOUT_FILENO)
        }
        fn begin_fd(fd: i32) -> io::Result<Self> {
            if STATE.with(|s| s.borrow().is_some()) {
                return Err(io::Error::other("QA_PHASE_TRACE_ACTIVE"));
            }
            let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(fd, info.as_mut_ptr()) } != 0 {
                return Err(io::Error::other("QA_PHASE_TRACE_FD"));
            }
            if unsafe { info.assume_init() }.st_mode & libc::S_IFMT != libc::S_IFIFO {
                return Err(io::Error::other("QA_PHASE_TRACE_PIPE"));
            }
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::other("QA_PHASE_TRACE_FLAGS"));
            }
            STATE.with(|s| {
                *s.borrow_mut() = Some(State {
                    fd,
                    sequence: 0,
                    failed: false,
                })
            });
            emit(1);
            Ok(Self {
                fd,
                flags,
                enabled: true,
                finished: false,
            })
        }
        pub(crate) fn finish(&mut self, success: bool) {
            if self.enabled && !self.finished {
                emit(if success { 2 } else { 3 });
                self.finished = true;
            }
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.enabled {
                if !self.finished {
                    emit(4);
                }
                STATE.with(|s| *s.borrow_mut() = None);
                unsafe {
                    libc::fcntl(self.fd, libc::F_SETFL, self.flags);
                }
            }
        }
    }
    pub(super) fn emit(code: u8) {
        STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            let Some(state) = state.as_mut() else {
                return;
            };
            if state.failed {
                return;
            }
            if state.sequence >= MAX_FRAMES
                || !matches!(code, 1..=4 | 10..=12 | 20..=22 | 30..=32 | 40..=41 | 50 | 52..=53)
            {
                state.failed = true;
                return;
            }
            let Ok(at) = crate::process_ownership::scope::monotonic_now() else {
                state.failed = true;
                return;
            };
            let mut frame = [0u8; 24];
            frame[..4].copy_from_slice(b"KIP1");
            frame[4] = code;
            frame[8..12].copy_from_slice(&state.sequence.to_le_bytes());
            frame[16..24].copy_from_slice(&at.to_le_bytes());
            // One nonblocking write; no retry, formatting, path or content.
            let count = unsafe { libc::write(state.fd, frame.as_ptr().cast(), frame.len()) };
            if count == frame.len() as isize {
                state.sequence += 1;
            } else {
                state.failed = true;
            }
        });
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{
            fs::File,
            io::Read,
            os::fd::{AsRawFd, FromRawFd},
        };
        fn pipe() -> (File, File) {
            let mut fds = [-1; 2];
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            for fd in fds {
                assert_eq!(
                    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                    0
                );
            }
            unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
        }
        #[test]
        fn installer_phase_trace_init_failure_preserves_handler_success() {
            let called = std::cell::Cell::new(0);
            let result = with_trace(
                || Guard::begin_fd(-1),
                || {
                    called.set(called.get() + 1);
                    Ok("original installer success")
                },
            );
            assert_eq!(called.get(), 1, "trace setup may not bypass the installer");
            assert_eq!(result.unwrap(), "original installer success");
            assert!(STATE.with(|state| state.borrow().is_none()));
        }
        #[test]
        fn installer_phase_trace_init_failure_preserves_original_handler_error() {
            let called = std::cell::Cell::new(0);
            let result: Result<(), crate::error::AppError> = with_trace(
                || Guard::begin_fd(-1),
                || {
                    called.set(called.get() + 1);
                    Err(crate::error::AppError::Conflict(
                        "original installer failure".into(),
                    ))
                },
            );
            assert_eq!(called.get(), 1, "trace setup may not bypass the installer");
            assert!(
                matches!(result, Err(crate::error::AppError::Conflict(message))
                if message == "original installer failure")
            );
            assert!(STATE.with(|state| state.borrow().is_none()));
        }
        #[test]
        fn installer_phase_trace_actual_atomic_write_is_fixed_metadata() {
            let (mut reader, writer) = pipe();
            let root = tempfile::tempdir().unwrap();
            let dir = crate::adapter_files::Dir::anchor(root.path()).unwrap();
            let mut guard = Guard::begin_fd(writer.as_raw_fd()).unwrap();
            dir.atomic(
                "PRIVATE_PATH_CANARY",
                b"PRIVATE_BODY_CANARY",
                0o600,
                Some(None),
                "test-flush",
            )
            .unwrap();
            guard.finish(true);
            drop(guard);
            drop(writer);
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            assert!(
                !bytes.is_empty(),
                "actual filesystem progress must be emitted"
            );
            assert_eq!(bytes.len() % 24, 0);
            let codes: Vec<_> = bytes
                .as_chunks::<24>()
                .0
                .iter()
                .enumerate()
                .map(|(i, frame)| {
                    assert_eq!(&frame[..4], b"KIP1");
                    assert_eq!(
                        u32::from_le_bytes(frame[8..12].try_into().unwrap()),
                        i as u32
                    );
                    assert_eq!(&frame[5..8], &[0; 3]);
                    assert_eq!(&frame[12..16], &[0; 4]);
                    assert!(u64::from_le_bytes(frame[16..24].try_into().unwrap()) > 0);
                    frame[4]
                })
                .collect();
            assert_eq!(codes, [1, 10, 11, 20, 21, 2]);
            assert!(!bytes.windows(8).any(|w| w == b"PRIVATE_"));
            assert_eq!(
                std::fs::read(root.path().join("PRIVATE_PATH_CANARY")).unwrap(),
                b"PRIVATE_BODY_CANARY"
            );
        }
        #[test]
        fn installer_phase_trace_full_pipe_failure_is_sticky_and_nonblocking() {
            let (reader, writer) = pipe();
            let mut guard = Guard::begin_fd(writer.as_raw_fd()).unwrap();
            let block = [0u8; 24];
            loop {
                let n =
                    unsafe { libc::write(writer.as_raw_fd(), block.as_ptr().cast(), block.len()) };
                if n < 0 {
                    assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
                    break;
                }
                assert!(n > 0);
            }
            emit(20);
            assert!(STATE.with(|s| s.borrow().as_ref().unwrap().failed));
            let sequence = STATE.with(|s| s.borrow().as_ref().unwrap().sequence);
            emit(21);
            guard.finish(false);
            assert_eq!(
                STATE.with(|s| s.borrow().as_ref().unwrap().sequence),
                sequence
            );
            drop(guard);
            drop(writer);
            drop(reader);
        }
        #[test]
        fn installer_phase_trace_frame_cap_never_wraps_or_resumes() {
            let (reader, writer) = pipe();
            let mut guard = Guard::begin_fd(writer.as_raw_fd()).unwrap();
            STATE.with(|s| s.borrow_mut().as_mut().unwrap().sequence = MAX_FRAMES);
            emit(20);
            assert!(STATE.with(|s| s.borrow().as_ref().unwrap().failed));
            emit(21);
            guard.finish(true);
            assert_eq!(
                STATE.with(|s| s.borrow().as_ref().unwrap().sequence),
                MAX_FRAMES
            );
            drop(guard);
            drop(writer);
            drop(reader);
        }
    }
}
