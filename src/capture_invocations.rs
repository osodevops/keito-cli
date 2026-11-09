//! Invocation safety flags, not accounting. No native identity or raw hook data.
use crate::error::AppError;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[cfg(unix)]
use std::{
    fs::File,
    io::{Read, Write},
    sync::Arc,
};
#[cfg(unix)]
pub const ACK: &[u8] = b"KEITO_CAPTURE_DURABLE_V1\n";
#[cfg(unix)]
const PENDING: &[u8] = b"KEITO_CAPTURE_PENDING_V1\n";
fn refused() -> AppError {
    AppError::Config("Capture invocation authority unavailable; collection remains paused".into())
}
#[cfg(unix)]
fn nonce_valid(nonce: &str) -> bool {
    nonce.len() == 64 && nonce.bytes().all(|b| b.is_ascii_hexdigit())
}
#[derive(Clone)]
pub struct Permit {
    #[cfg(unix)]
    nonce: String,
    // Pin the verified live inode through validation/commit/ACK and every clone.
    // A read-only descriptor owns no guardian flock and closes across exec.
    #[cfg(unix)]
    marker: Arc<File>,
}
pub struct Marker {
    pub nonce: String,
    pub live: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    preparing: bool,
}
/// Capture state follows effective-user passwd HOME, never a hook's HOME/CWD.
pub fn directory() -> Result<PathBuf, AppError> {
    if let Some(path) = std::env::var_os("KEITO_CAPTURE_HOME") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        use std::os::unix::ffi::OsStrExt;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let code = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if code != 0 || result.is_null() {
            return Err(refused());
        }
        let entry = unsafe { entry.assume_init() };
        if entry.pw_dir.is_null() {
            return Err(refused());
        }
        let home = PathBuf::from(std::ffi::OsStr::from_bytes(
            unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes(),
        ));
        if !home.is_absolute() || home == Path::new("/") {
            return Err(refused());
        }
        #[cfg(target_os = "macos")]
        {
            Ok(home.join("Library/Application Support/keito/capture"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(home.join(".config/keito/capture"))
        }
    }
    #[cfg(not(unix))]
    {
        Err(refused())
    }
}
#[cfg(unix)]
fn private_directory(directory: &Path, private_components: usize) -> Result<File, AppError> {
    use std::ffi::CString;
    use std::os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    };
    if !directory.is_absolute()
        || directory
            .as_os_str()
            .to_str()
            .is_none_or(|p| p.split('/').any(|c| c == "." || c == ".."))
    {
        return Err(refused());
    }
    let name = CString::new("/").map_err(|_| refused())?;
    let fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(refused());
    }
    let mut parent = unsafe { OwnedFd::from_raw_fd(fd) };
    let parts: Vec<_> = directory
        .components()
        .filter_map(|c| {
            if let std::path::Component::Normal(n) = c {
                Some(n)
            } else {
                None
            }
        })
        .collect();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(part.as_bytes()).map_err(|_| refused())?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(refused());
        }
        parent = unsafe { OwnedFd::from_raw_fd(fd) };
        let metadata = File::from(parent.try_clone().map_err(|_| refused())?)
            .metadata()
            .map_err(|_| refused())?;
        if metadata.uid() != 0 && metadata.uid() != unsafe { libc::geteuid() } {
            return Err(refused());
        }
        if index + private_components >= parts.len()
            && (metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o777 != 0o700)
        {
            return Err(refused());
        }
    }
    Ok(File::from(parent))
}
#[cfg(unix)]
fn root(directory: &Path) -> Result<File, AppError> {
    private_directory(&directory.join("capture-invocations"), 2)
}
#[cfg(unix)]
fn publication(directory: &Path) -> Result<crate::capture_authority::AuthorityGuard, AppError> {
    crate::capture_authority::AuthorityGuard::publication(&private_directory(directory, 1)?)
}
#[cfg(unix)]
fn names(root: &File) -> Result<Vec<String>, AppError> {
    use std::ffi::CStr;
    use std::os::fd::AsRawFd;
    let fd = unsafe { libc::dup(root.as_raw_fd()) };
    if fd < 0 {
        return Err(refused());
    }
    let dir = unsafe { libc::fdopendir(fd) };
    if dir.is_null() {
        unsafe { libc::close(fd) };
        return Err(refused());
    }
    let mut result = Vec::new();
    let failed;
    loop {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            #[cfg(target_os = "macos")]
            {
                failed = unsafe { *libc::__error() } != 0;
            }
            #[cfg(target_os = "linux")]
            {
                failed = unsafe { *libc::__errno_location() } != 0;
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                failed = true;
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let Ok(name) = name.to_str() else {
            failed = true;
            break;
        };
        if matches!(name, "." | "..") {
            continue;
        }
        if result.len() >= 1024 {
            failed = true;
            break;
        }
        result.push(name.to_owned());
    }
    unsafe { libc::closedir(dir) };
    if failed {
        Err(refused())
    } else {
        Ok(result)
    }
}
#[cfg(unix)]
fn inspect(root: &File, nonce: &str, preparing: bool) -> Result<Marker, AppError> {
    // Snapshot/dead probes must release any successful probe flock immediately.
    Ok(inspect_file(root, nonce, preparing)?.0)
}
#[cfg(unix)]
fn inspect_file(root: &File, nonce: &str, preparing: bool) -> Result<(Marker, File), AppError> {
    use std::ffi::CString;
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    };
    if !nonce_valid(nonce) {
        return Err(refused());
    }
    let name = CString::new(format!(
        "{nonce}.{}",
        if preparing { "preparing" } else { "pending" }
    ))
    .map_err(|_| refused())?;
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(refused());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let m = file.metadata().map_err(|_| refused())?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
        || m.len() > PENDING.len() as u64
        || (!preparing && m.len() != PENDING.len() as u64)
    {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take((PENDING.len() + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| refused())?;
    if (!preparing && bytes != PENDING) || (preparing && !PENDING.starts_with(&bytes)) {
        return Err(refused());
    }
    let code = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    let live = if code == 0 {
        false
    } else if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
        true
    } else {
        return Err(refused());
    };
    Ok((
        Marker {
            nonce: nonce.to_owned(),
            live,
            device: m.dev(),
            inode: m.ino(),
            preparing,
        },
        file,
    ))
}
pub fn snapshot(directory: &Path) -> Result<Vec<Marker>, AppError> {
    let path = directory.join("capture-invocations");
    let exists = |path: &Path| match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(refused()),
    };
    if !exists(&path)? && !exists(&directory.join("capture-invocations-publication.lock"))? {
        return Ok(vec![]);
    }
    #[cfg(unix)]
    {
        let _publication = publication(directory)?;
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err(refused()),
            Ok(_) => (),
        }
        let root = root(directory)?;
        let mut markers = Vec::new();
        for name in names(&root)? {
            let (nonce, preparing) = if let Some(n) = name.strip_suffix(".pending") {
                (n, false)
            } else if let Some(n) = name.strip_suffix(".preparing") {
                (n, true)
            } else {
                return Err(refused());
            };
            markers.push(inspect(&root, nonce, preparing)?);
        }
        Ok(markers)
    }
    #[cfg(not(unix))]
    {
        Err(refused())
    }
}
pub fn protect_ack(fd: i32) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        if !(3..=1024).contains(&fd) {
            return Err(refused());
        }
        let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, info.as_mut_ptr()) } != 0 {
            return Err(refused());
        }
        if unsafe { info.assume_init() }.st_mode & libc::S_IFMT != libc::S_IFIFO {
            return Err(refused());
        }
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(refused());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = fd;
        Err(refused())
    }
}
impl Permit {
    pub fn new(directory: &Path, nonce: &str) -> Result<Self, AppError> {
        #[cfg(unix)]
        {
            let _publication = publication(directory)?;
            let (m, file) = inspect_file(&root(directory)?, nonce, false)?;
            if !m.live {
                return Err(refused());
            }
            Ok(Self {
                nonce: m.nonce,
                marker: Arc::new(file),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (directory, nonce);
            Err(refused())
        }
    }
    pub fn validate(&self, directory: &Path) -> Result<(), AppError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let expected = self.marker.metadata().map_err(|_| refused())?;
            if expected.nlink() != 1 {
                return Err(refused());
            }
            let current = Self::new(directory, &self.nonce)?;
            let observed = current.marker.metadata().map_err(|_| refused())?;
            if observed.dev() != expected.dev() || observed.ino() != expected.ino() {
                return Err(refused());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = directory;
            Err(refused())
        }
    }
    pub fn acknowledge(&self, directory: &Path, fd: i32) -> Result<(), AppError> {
        self.validate(directory)?;
        #[cfg(unix)]
        {
            use std::os::fd::FromRawFd;
            if !(3..=1024).contains(&fd) {
                return Err(refused());
            }
            let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(fd, info.as_mut_ptr()) } != 0 {
                return Err(refused());
            }
            let info = unsafe { info.assume_init() };
            if info.st_mode & libc::S_IFMT != libc::S_IFIFO {
                return Err(refused());
            }
            let mut pipe = unsafe { File::from_raw_fd(fd) };
            pipe.write_all(ACK).map_err(|_| refused())?;
            pipe.flush().map_err(|_| refused())?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = fd;
            Err(refused())
        }
    }
}
pub fn remove_dead(directory: &Path, markers: &[Marker]) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        if markers.is_empty() {
            return Ok(());
        }
        let _publication = publication(directory)?;
        let root = root(directory)?;
        for marker in markers {
            let current = inspect(&root, &marker.nonce, marker.preparing)?;
            if current.live || current.device != marker.device || current.inode != marker.inode {
                return Err(refused());
            }
            let name = CString::new(format!(
                "{}.{}",
                marker.nonce,
                if marker.preparing {
                    "preparing"
                } else {
                    "pending"
                }
            ))
            .map_err(|_| refused())?;
            if unsafe { libc::unlinkat(root.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(refused());
            }
        }
        root.sync_all().map_err(|_| refused())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, markers);
        Err(refused())
    }
}

#[cfg(all(test, unix))]
mod permit_tests {
    use super::*;
    use std::os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    fn marker(root: &Path, nonce: &str, live: bool) -> File {
        let directory = root.join("capture-invocations");
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(format!("{nonce}.pending")))
            .unwrap();
        file.write_all(PENDING).unwrap();
        file.sync_all().unwrap();
        if live {
            assert_eq!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
        }
        file
    }

    #[test]
    fn live_permit_pins_readonly_cloexec_inode_until_last_clone_without_keeping_guardian_alive() {
        use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let nonce = crate::capture::hash("pinned-permit");
        let guard = marker(&root, &nonce, true);
        let identity = guard.metadata().unwrap();
        let permit = Permit::new(&root, &nonce).unwrap();
        let cloned = permit.clone();
        assert!(permit.validate(&root).is_ok());
        drop(guard);
        // Retaining a read descriptor must not retain the guardian's exclusive lock.
        assert!(permit.validate(&root).is_err());
        let dead = snapshot(&root).unwrap();
        assert_eq!(dead.len(), 1);
        assert!(!dead[0].live);
        remove_dead(&root, &dead).unwrap();
        assert_eq!(
            fs::read_dir(root.join("capture-invocations"))
                .unwrap()
                .count(),
            0
        );
        let matching = || {
            fs::read_dir("/dev/fd")
                .unwrap()
                .filter_map(|entry| {
                    let fd = entry.ok()?.file_name().to_str()?.parse::<i32>().ok()?;
                    let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
                    if unsafe { libc::fstat(fd, info.as_mut_ptr()) } != 0 {
                        return None;
                    }
                    let info = unsafe { info.assume_init() };
                    #[cfg(target_os = "macos")]
                    let device = info.st_dev as u64;
                    #[cfg(not(target_os = "macos"))]
                    let device = info.st_dev;
                    if device != identity.dev() || info.st_ino != identity.ino() {
                        return None;
                    }
                    Some((unsafe { libc::fcntl(fd, libc::F_GETFD) }, unsafe {
                        libc::fcntl(fd, libc::F_GETFL)
                    }))
                })
                .collect::<Vec<_>>()
        };
        let held = matching();
        assert!(
            !held.is_empty(),
            "accepted Permit dropped its verified descriptor, permitting inode ABA"
        );
        assert!(held
            .iter()
            .all(|(descriptor, access)| descriptor & libc::FD_CLOEXEC != 0
                && access & libc::O_ACCMODE == libc::O_RDONLY));
        let replacement = marker(&root, &nonce, true);
        assert_ne!(
            (
                replacement.metadata().unwrap().dev(),
                replacement.metadata().unwrap().ino()
            ),
            (identity.dev(), identity.ino())
        );
        assert!(permit.validate(&root).is_err());
        drop(permit);
        assert!(!matching().is_empty(), "Permit clone lost the pin");
        drop(cloned);
        assert!(
            matching().is_empty(),
            "last Permit clone leaked its descriptor"
        );
        // Keep the replacement's guardian live until every pin assertion completes.
        assert!(replacement.as_raw_fd() >= 0);
    }
}
