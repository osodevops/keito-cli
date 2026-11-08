//! Cross-process refusal authority. Acquire SQLite first, then this bounded lock.
//! Marker publication never waits for the lock and uses immutable unique files.
use crate::error::AppError;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};
fn refused() -> AppError {
    AppError::Config("Capture authority unavailable; collection remains paused".into())
}
pub struct AuthorityGuard {
    _file: File,
}
impl AuthorityGuard {
    #[cfg(unix)]
    pub fn publication(parent: &File) -> Result<Self, AppError> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = std::ffi::CString::new("capture-invocations-publication.lock")
            .map_err(|_| refused())?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(refused());
        }
        Self::lock(unsafe { File::from_raw_fd(fd) }, true)
    }
    #[cfg(unix)]
    pub fn acquire(directory: &Path, wait: bool) -> Result<Self, AppError> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join("capture-authority.lock"))
            .map_err(|_| refused())?;
        Self::lock(file, wait)
    }
    #[cfg(unix)]
    fn lock(file: File, wait: bool) -> Result<Self, AppError> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let metadata = file.metadata().map_err(|_| refused())?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(refused());
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| refused())?;
        let start = Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self { _file: file });
            }
            let error = std::io::Error::last_os_error();
            if !wait
                || error.kind() != std::io::ErrorKind::WouldBlock
                || start.elapsed() >= Duration::from_millis(25)
            {
                return Err(refused());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[cfg(not(unix))]
    pub fn acquire(_directory: &Path, _wait: bool) -> Result<Self, AppError> {
        Err(refused())
    }
}
