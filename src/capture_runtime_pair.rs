//! Release-pair identity used by native adapter installation.
//!
//! This descriptor is transport data, not evidence of trust by itself. Production
//! callers must obtain it from the compiled release-anchor verifier. A local
//! manifest or a same-directory executable never establishes release identity.
use serde::Serialize;
use std::path::PathBuf;

pub const GUARD_CONTRACT: u32 = 1;
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

use crate::{capture, error::AppError};
use serde::Deserialize;
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
fn read_file(path: &Path, max: u64, executable: bool) -> Result<Vec<u8>, AppError> {
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
    let mut bytes = Vec::new();
    let mut bounded = (&file).take(max + 1);
    bounded.read_to_end(&mut bytes).map_err(|_| refused())?;
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
    if bytes.len() as u64 > max
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
    Ok(bytes)
}
#[cfg(not(unix))]
fn read_file(_: &Path, _: u64, _: bool) -> Result<Vec<u8>, AppError> {
    Err(refused())
}
fn hash_file(path: &Path) -> Result<String, AppError> {
    Ok(capture::hash(read_file(path, 128 * 1024 * 1024, true)?))
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
    let core_sha256 = hash_file(core)?;
    let guard_sha256 = hash_file(&guard)?;
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
            .filter(|v| (4..=1024).contains(v))
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
