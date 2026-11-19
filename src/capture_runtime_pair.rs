//! Release-pair identity used by native adapter installation.
//!
//! This descriptor is transport data, not evidence of trust by itself. Production
//! callers must obtain it from the compiled release-anchor verifier. A local
//! manifest or a same-directory executable never establishes release identity.
use serde::Serialize;
use std::path::PathBuf;

pub const GUARD_CONTRACT: u32 = 2;
#[cfg(unix)]
static IMAGE_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CaptureRuntimeIdentity {
    pub core_path: PathBuf,
    pub guard_path: PathBuf,
    pub core_sha256: String,
    pub guard_sha256: String,
    pub version: String,
    pub target: String,
    pub guard_contract: u32,
}

use crate::error::AppError;
use serde::Deserialize;
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
#[cfg(unix)]
use std::{fs, io::Read, path::Component};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    manifest_contract: u32,
    version: String,
    target: String,
    guard_contract: u32,
    files: BTreeMap<String, String>,
}
fn refused() -> AppError {
    AppError::Config("Capture requires an intact official keito / keito-capture-guard release pair; manual CLI commands remain available".into())
}
#[cfg(unix)]
fn read_file_with<T>(
    path: &Path,
    max: u64,
    executable: bool,
    consume: impl FnOnce(&fs::File, u64) -> Result<(T, u64), AppError>,
) -> Result<T, AppError> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
    };
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err(refused());
    }
    let fd = unsafe {
        libc::open(
            CString::new("/").unwrap().as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(refused());
    }
    let mut parent = unsafe { OwnedFd::from_raw_fd(fd) };
    let parts: Vec<_> = path
        .components()
        .filter_map(|p| {
            if let Component::Normal(p) = p {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    let (leaf, ancestors) = parts.split_last().ok_or_else(refused)?;
    for part in ancestors {
        let name = CString::new(part.as_bytes()).map_err(|_| refused())?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(refused());
        }
        parent = unsafe { OwnedFd::from_raw_fd(fd) };
    }
    let name = CString::new(leaf.as_bytes()).map_err(|_| refused())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(refused());
    }
    let file = unsafe { fs::File::from_raw_fd(fd) };
    let before = file.metadata().map_err(|_| refused())?;
    if !before.is_file()
        || before.len() > max
        || before.mode() & 0o022 != 0
        || (executable && before.mode() & 0o111 == 0)
        || before.uid() != 0 && before.uid() != unsafe { libc::geteuid() }
    {
        return Err(refused());
    }
    let (output, bytes_read) = consume(&file, max)?;
    let after = file.metadata().map_err(|_| refused())?;
    let mut current = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            current.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(refused());
    }
    let current = unsafe { current.assume_init() };
    if bytes_read > max
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mode() != after.mode()
        || before.uid() != after.uid()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || current.st_dev as u128 != u128::from(before.dev())
        || u128::from(current.st_ino) != u128::from(before.ino())
    {
        return Err(refused());
    }
    Ok(output)
}
#[cfg(unix)]
fn read_file(path: &Path, max: u64, executable: bool) -> Result<Vec<u8>, AppError> {
    read_file_with(path, max, executable, |file, max| {
        let mut bytes = Vec::new();
        let mut bounded = file.take(max + 1);
        bounded.read_to_end(&mut bytes).map_err(|_| refused())?;
        let length = bytes.len() as u64;
        Ok((bytes, length))
    })
}
#[cfg(not(unix))]
fn read_file(_: &Path, _: u64, _: bool) -> Result<Vec<u8>, AppError> {
    Err(refused())
}
#[cfg(unix)]
fn hash_reader(reader: &mut impl Read, max: u64) -> Result<(String, u64), AppError> {
    let mut hash = Sha256::new();
    let mut bytes_read = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    let mut bounded = reader.take(max + 1);
    loop {
        let count = match bounded.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(refused()),
        };
        if count == 0 {
            break;
        }
        bytes_read = bytes_read.checked_add(count as u64).ok_or_else(refused)?;
        if bytes_read > max {
            return Err(refused());
        }
        hash.update(&buffer[..count]);
    }
    let digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((digest, bytes_read))
}
#[cfg(unix)]
fn hash_file(path: &Path) -> Result<String, AppError> {
    read_file_with(path, 128 * 1024 * 1024, true, |file, max| {
        // The SAME retained descriptor and pre/post path+inode gates still
        // verify every image byte. Only the whole-image Vec is removed.
        hash_reader(&mut &*file, max)
    })
}
#[cfg(not(unix))]
fn hash_file(path: &Path) -> Result<String, AppError> {
    Ok(crate::capture::hash(read_file(
        path,
        128 * 1024 * 1024,
        true,
    )?))
}
/// Pure filesystem qualification against a caller-supplied compiled identity.
/// The release command uses only its embedded anchor, never manifest trust.
pub fn verify(
    core: &Path,
    guard_hash: &str,
    version: &str,
    target: &str,
) -> Result<CaptureRuntimeIdentity, AppError> {
    if guard_hash.len() != 64
        || !guard_hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(refused());
    }
    let parent = core.parent().ok_or_else(refused)?;
    let guard = parent.join("keito-capture-guard");
    let core_sha256 = crate::capture_profile_measure!(CoreHash, hash_file(core))?;
    let guard_sha256 = crate::capture_profile_measure!(GuardHash, hash_file(&guard))?;
    if guard_sha256 != guard_hash {
        return Err(refused());
    }
    let path = parent.join("capture-runtime-manifest.json");
    let manifest: Manifest =
        serde_json::from_slice(&read_file(&path, 16384, false)?).map_err(|_| refused())?;
    if manifest.manifest_contract != 1
        || manifest.guard_contract != GUARD_CONTRACT
        || manifest.version != version
        || manifest.target != target
        || manifest.files.len() != 2
        || manifest.files.get("keito") != Some(&core_sha256)
        || manifest.files.get("keito-capture-guard") != Some(&guard_sha256)
    {
        return Err(refused());
    }
    Ok(CaptureRuntimeIdentity {
        core_path: core.into(),
        guard_path: guard,
        core_sha256,
        guard_sha256,
        version: version.into(),
        target: target.into(),
        guard_contract: GUARD_CONTRACT,
    })
}
/// Must run synchronously before constructing Tokio or parsing CLI input.
/// Only stdio, ACK, and this read-only image descriptor cross kernel exec.
/// The image descriptor is closed before this function returns, including errors.
pub fn bootstrap_image() -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::MetadataExt,
        };
        let Some(raw) = std::env::var_os("KEITO_CAPTURE_IMAGE_FD") else {
            return Ok(());
        };
        let fd = raw
            .to_str()
            .and_then(|v| v.parse::<i32>().ok())
            .filter(|v| (5..=1024).contains(v))
            .ok_or_else(refused)?;
        let file = unsafe { fs::File::from_raw_fd(fd) };
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(refused());
        }
        let metadata = file.metadata().map_err(|_| refused())?;
        if !metadata.is_file()
            || unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_ACCMODE != libc::O_RDONLY
        {
            return Err(refused());
        }
        #[cfg(target_os = "macos")]
        let path = {
            use std::{ffi::CStr, os::unix::ffi::OsStrExt};
            let mut buffer = [0i8; libc::PATH_MAX as usize];
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } < 0 {
                return Err(refused());
            }
            PathBuf::from(std::ffi::OsStr::from_bytes(
                unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes(),
            ))
        };
        #[cfg(not(target_os = "macos"))]
        let path =
            fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(|_| refused())?;
        let actual = fs::symlink_metadata(&path).map_err(|_| refused())?;
        if !path.is_absolute()
            || actual.file_type().is_symlink()
            || actual.dev() != metadata.dev()
            || actual.ino() != metadata.ino()
        {
            return Err(refused());
        }
        let running = std::env::current_exe().map_err(|_| refused())?;
        let running_metadata = fs::metadata(&running).map_err(|_| refused())?;
        if running_metadata.dev() != metadata.dev() || running_metadata.ino() != metadata.ino() {
            return Err(refused());
        }
        drop(file);
        IMAGE_PATH.set(path).map_err(|_| refused())?;
        std::env::remove_var("KEITO_CAPTURE_IMAGE_FD");
        Ok(())
    }
    #[cfg(not(unix))]
    {
        if std::env::var_os("KEITO_CAPTURE_IMAGE_FD").is_some() {
            Err(refused())
        } else {
            Ok(())
        }
    }
}
pub fn current() -> Result<CaptureRuntimeIdentity, AppError> {
    #[cfg(unix)]
    {
        let anchor = option_env!("KEITO_RELEASE_GUARD_SHA256").ok_or_else(refused)?;
        let core = if let Some(path) = IMAGE_PATH.get() {
            path.clone()
        } else {
            std::env::current_exe().map_err(|_| refused())?
        };
        verify(
            &core,
            anchor,
            env!("CARGO_PKG_VERSION"),
            env!("KEITO_BUILD_TARGET"),
        )
    }
    #[cfg(not(unix))]
    {
        Err(refused())
    }
}

/// Sealed actual verifier result for a normal guarded hook. The public runtime
/// descriptor remains transport data; callers cannot deserialize this proof.
#[cfg(unix)]
pub(crate) struct HookRuntime {
    identity: CaptureRuntimeIdentity,
    keeper: crate::process_ownership::image::VerifiedKeeperImage,
}
#[cfg(unix)]
impl HookRuntime {
    pub(crate) fn current(expected_core: &str) -> Result<Self, AppError> {
        let identity = current()?;
        if identity.core_sha256 != expected_core {
            return Err(refused());
        }
        let keeper = crate::process_ownership::image::VerifiedKeeperImage::open(
            &identity.guard_path,
            &identity.guard_sha256,
        )
        .map_err(|_| refused())?;
        Ok(Self { identity, keeper })
    }
    pub(crate) fn identity(&self) -> &CaptureRuntimeIdentity {
        &self.identity
    }
    pub(crate) fn keeper(&self) -> &crate::process_ownership::image::VerifiedKeeperImage {
        &self.keeper
    }
}

#[cfg(all(test, unix))]
mod streaming_hash_tests {
    use super::*;
    use std::io::{self, Cursor};
    #[test]
    fn stream_digest_matches_exact_whole_buffer_sha256_at_chunk_boundaries() {
        for length in [0, 1, 65535, 65536, 65537, 131073] {
            let bytes: Vec<u8> = (0..length).map(|n| (n % 251) as u8).collect();
            let (digest, count) = hash_reader(&mut Cursor::new(&bytes), length as u64).unwrap();
            assert_eq!(digest, crate::capture::hash(&bytes));
            assert_eq!(count, length as u64);
        }
    }
    #[test]
    fn excess_byte_is_refused_instead_of_accepting_a_hashed_prefix() {
        assert!(hash_reader(&mut Cursor::new(b"a"), 0).is_err());
        assert!(hash_reader(&mut Cursor::new(b"abc"), 2).is_err());
        let (digest, count) = hash_reader(&mut Cursor::new(b"ab"), 2).unwrap();
        assert_eq!(digest, crate::capture::hash(b"ab"));
        assert_eq!(count, 2);
    }
    struct InterruptedThenChunks {
        bytes: Cursor<Vec<u8>>,
        interrupted: bool,
    }
    impl Read for InterruptedThenChunks {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let end = output.len().min(3);
            self.bytes.read(&mut output[..end])
        }
    }
    #[test]
    fn interrupted_and_partial_reads_preserve_exact_digest_and_count() {
        let bytes = b"complete image bytes after interruption".to_vec();
        let mut reader = InterruptedThenChunks {
            bytes: Cursor::new(bytes.clone()),
            interrupted: false,
        };
        let (digest, count) = hash_reader(&mut reader, bytes.len() as u64).unwrap();
        assert_eq!(digest, crate::capture::hash(&bytes));
        assert_eq!(count, bytes.len() as u64);
    }
    struct FailsAfterPrefix(bool);
    impl Read for FailsAfterPrefix {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                return Err(io::Error::from(io::ErrorKind::Other));
            }
            self.0 = true;
            output[0] = b'x';
            Ok(1)
        }
    }
    #[test]
    fn a_read_fault_never_returns_a_partial_digest() {
        assert!(hash_reader(&mut FailsAfterPrefix(false), 10).is_err());
    }
}
