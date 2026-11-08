//! Descriptor-relative, no-follow filesystem operations for adapter publication.
//! Errors intentionally omit local paths and file contents.
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
        self.file.sync_all().map_err(|_| failure())
    }
    pub(crate) fn atomic(
        &self,
        child: &str,
        bytes: &[u8],
        mode: u32,
        expected: Option<Option<&[u8]>>,
        phase: &str,
    ) -> Result<(), AppError> {
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
                .and_then(|_| f.sync_all())
                .map_err(|_| failure())?;
            checkpoint(phase);
            publication_fault(phase)?;
            self.verify()?;
            if let Some(expected) = expected {
                if self.read(child, 32 * 1024 * 1024)?.as_deref() != expected {
                    return Err(conflict());
                }
            }
            self.rename(&temp, self, child)?;
            self.sync()
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
        self.sync()?;
        dest.sync()
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
        let dir = self.child(child, false)?;
        for entry in dir.entries()? {
            if dir.child(&entry, false).is_ok() {
                dir.remove_tree(&entry)?;
            } else {
                let _ = dir.open(&entry)?;
                dir.unlink(&entry, false)?;
            }
        }
        self.unlink(child, true)
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
