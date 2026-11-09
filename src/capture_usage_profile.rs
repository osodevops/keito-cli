//! Explicit local numeric opt-in configuration. Functional paths are private
//! device configuration, never usage evidence, SQLite policy, diagnostics or
//! outbox fields. This module does not read a transcript or grant hook authority.
#![cfg(unix)]
use crate::{
    adapter_files::{digest, hex, nonce, Dir},
    capture::Binding,
    capture_runtime_pair::CaptureRuntimeIdentity,
    cli::skill::SkillAgent,
    error::AppError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{self, File, Metadata},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt},
    },
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const MAX_PROFILE_BYTES: usize = 64 * 1024;
// Enable-only executable admission. The supported Claude native image is
//321,995,200bytes; stream a bounded image once, never hash it in a hook.
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
fn refused() -> AppError {
    AppError::Config("Numeric profile qualification is unavailable; review the supported native version, managed installation and explicit profile approval. Metadata capture remains available.".into())
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectoryStamp {
    device: u64,
    inode: u64,
    owner: u32,
    mode: u32,
    created_seconds: u64,
    created_nanos: u32,
}
impl DirectoryStamp {
    fn observe(path: &Path) -> Result<Self, AppError> {
        let metadata = walk(path, true)?.metadata().map_err(|_| refused())?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(refused());
        }
        // Ordinary child creation changes directory mtime/ctime. Birth time
        // instead distinguishes a removed/recreated root even after inode reuse.
        // Filesystems without this identity component stay unqualified.
        let created = metadata
            .created()
            .map_err(|_| refused())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| refused())?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner: metadata.uid(),
            mode: metadata.mode(),
            created_seconds: created.as_secs(),
            created_nanos: created.subsec_nanos(),
        })
    }
    fn matches(&self, path: &Path) -> bool {
        Self::observe(path).is_ok_and(|current| {
            current.device == self.device
                && current.inode == self.inode
                && current.owner == self.owner
                && current.mode == self.mode
                && current.created_seconds == self.created_seconds
                && current.created_nanos == self.created_nanos
        })
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileStamp {
    device: u64,
    inode: u64,
    owner: u32,
    mode: u32,
    links: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanos: i64,
    changed_seconds: i64,
    changed_nanos: i64,
}
impl FileStamp {
    fn from_metadata(metadata: &Metadata) -> Result<Self, AppError> {
        let owner = metadata.uid();
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o022 != 0
            || (owner != 0 && owner != unsafe { libc::geteuid() })
        {
            return Err(refused());
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner,
            mode: metadata.mode(),
            links: metadata.nlink(),
            bytes: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanos: metadata.ctime_nsec(),
        })
    }
    fn observe(path: &Path) -> Result<Self, AppError> {
        Self::from_metadata(&walk(path, false)?.metadata().map_err(|_| refused())?)
    }
    pub(crate) fn matches_metadata(&self, metadata: &Metadata) -> bool {
        Self::from_metadata(metadata).is_ok_and(|current| {
            current.device == self.device
                && current.inode == self.inode
                && current.owner == self.owner
                && current.mode == self.mode
                && current.links == self.links
                && current.bytes == self.bytes
                && current.modified_seconds == self.modified_seconds
                && current.modified_nanos == self.modified_nanos
                && current.changed_seconds == self.changed_seconds
                && current.changed_nanos == self.changed_nanos
        })
    }
    fn matches(&self, path: &Path) -> bool {
        walk(path, false)
            .and_then(|file| file.metadata().map_err(|_| refused()))
            .is_ok_and(|metadata| self.matches_metadata(&metadata))
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FilePin {
    path: PathBuf,
    stamp: Option<FileStamp>,
}
impl FilePin {
    fn observe(path: PathBuf, optional: bool) -> Result<Self, AppError> {
        let missing = fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        let stamp = if optional && missing {
            None
        } else {
            Some(FileStamp::observe(&path)?)
        };
        Ok(Self { path, stamp })
    }
    fn matches(&self) -> bool {
        match &self.stamp {
            Some(stamp) => stamp.matches(&self.path),
            None => fs::symlink_metadata(&self.path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImagePin {
    path: PathBuf,
    stamp: FileStamp,
    sha256: String,
}
impl ImagePin {
    fn observe(path: &Path, native: bool) -> Result<Self, AppError> {
        canonical(path)?;
        let mut file = walk(path, false)?;
        let stamp = FileStamp::from_metadata(&file.metadata().map_err(|_| refused())?)?;
        if stamp.mode & 0o111 == 0 || stamp.bytes > MAX_IMAGE_BYTES {
            return Err(refused());
        }
        let mut hasher = Sha256::new();
        let mut block = [0u8; 64 * 1024];
        let mut prefix = Vec::with_capacity(4);
        let mut total = 0u64;
        loop {
            let count = file.read(&mut block).map_err(|_| refused())?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_IMAGE_BYTES {
                return Err(refused());
            }
            if prefix.len() < 4 {
                prefix.extend_from_slice(&block[..count.min(4 - prefix.len())]);
            }
            if native && prefix.len() == 4 && !native_image(&prefix) {
                return Err(refused());
            }
            hasher.update(&block[..count]);
        }
        if total != stamp.bytes || !stamp.matches_metadata(&file.metadata().map_err(|_| refused())?)
        {
            return Err(refused());
        }
        if native && !native_image(&prefix) {
            return Err(refused());
        }
        Ok(Self {
            path: path.to_owned(),
            stamp,
            sha256: hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        })
    }
    fn matches(&self) -> bool {
        hex(&self.sha256, 64) && self.stamp.matches(&self.path)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProfileRecord {
    schema_version: u8,
    profile_id: String,
    source_namespace: String,
    harness_version: String,
    home: PathBuf,
    home_stamp: DirectoryStamp,
    profile_root: PathBuf,
    profile_stamp: DirectoryStamp,
    managed_root: PathBuf,
    managed_stamp: DirectoryStamp,
    generation_root: PathBuf,
    generation_stamp: DirectoryStamp,
    registration_digest: String,
    runtime_core_sha256: String,
    runtime_guard_sha256: String,
    launcher: ImagePin,
    native: ImagePin,
    files: Vec<FilePin>,
}

pub(crate) struct PreparedProfile {
    id: String,
    digest: String,
    binding_id: String,
    binding_version: i64,
    namespace: String,
    record: ProfileRecord,
    record_path: PathBuf,
    record_stamp: FileStamp,
}
impl PreparedProfile {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }
    pub(crate) fn matches(&self, binding: &Binding, namespace: &str) -> bool {
        self.binding_id == binding.id
            && self.binding_version == binding.version
            && self.namespace == namespace
            && self.record_stamp.matches(&self.record_path)
            && self.record.home_stamp.matches(&self.record.home)
            && self.record.profile_stamp.matches(&self.record.profile_root)
            && self.record.managed_stamp.matches(&self.record.managed_root)
            && self
                .record
                .generation_stamp
                .matches(&self.record.generation_root)
            && self.record.launcher.matches()
            && self.record.native.matches()
            && self.record.files.iter().all(FilePin::matches)
    }
}

/// Loaded only after fresh transactional persisted consent. Holding this value
/// grants no source access by itself; the caller must also prove a live guarded
/// invocation and actual bounded native origin, then exact active session/path.
pub(crate) struct ProfileLease {
    record: ProfileRecord,
    record_path: PathBuf,
    record_stamp: FileStamp,
    _record_file: File,
}
impl ProfileLease {
    pub(crate) fn load(
        directory: &Path,
        policy: &crate::capture_usage_consent::ConsentPolicy,
        runtime: &CaptureRuntimeIdentity,
    ) -> Result<Self, &'static str> {
        #[cfg(test)]
        crate::capture_usage_test_io::observe(0);
        if !policy.enabled || !hex(&policy.profile_id, 64) || !hex(&policy.profile_digest, 64) {
            return Err("NUMERIC_CAPTURE_DISABLED");
        }
        let path = directory
            .join("profiles")
            .join(format!("{}.json", policy.profile_id));
        let mut file = walk(&path, false).map_err(|_| "PROFILE_UNAVAILABLE")?;
        let metadata = file.metadata().map_err(|_| "PROFILE_UNAVAILABLE")?;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() > MAX_PROFILE_BYTES as u64
        {
            return Err("PROFILE_UNAVAILABLE");
        }
        let stamp = FileStamp::from_metadata(&metadata).map_err(|_| "PROFILE_UNAVAILABLE")?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take((MAX_PROFILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "PROFILE_UNAVAILABLE")?;
        if bytes.len() > MAX_PROFILE_BYTES
            || digest(&bytes) != policy.profile_digest
            || !stamp.matches_metadata(&file.metadata().map_err(|_| "PROFILE_CHANGED")?)
        {
            return Err("PROFILE_CHANGED");
        }
        let record: ProfileRecord =
            serde_json::from_slice(&bytes).map_err(|_| "PROFILE_UNAVAILABLE")?;
        let (host, source, version) = match policy.source_namespace.as_str() {
            "claude-code" => (".claude", "projects", "2.1.319"),
            "codex" => (".codex", "sessions", "0.167.0"),
            _ => return Err("UNSUPPORTED_SOURCE"),
        };
        if record.schema_version != 1
            || record.profile_id != policy.profile_id
            || record.source_namespace != policy.source_namespace
            || record.harness_version != version
            || record.profile_root != record.home.join(host)
            || record.managed_root != record.profile_root.join(source)
            || record.files.len() > 70
            || !hex(&record.registration_digest, 64)
            || !hex(&record.launcher.sha256, 64)
            || !hex(&record.native.sha256, 64)
        {
            return Err("PROFILE_UNAVAILABLE");
        }
        let parent = record.profile_root.join(".keito-capture-generations");
        if record.generation_root.parent() != Some(parent.as_path())
            || record
                .generation_root
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !hex(name, 64))
        {
            return Err("REGISTRATION_CHANGED");
        }
        let lease = Self {
            record,
            record_path: path,
            record_stamp: stamp,
            _record_file: file,
        };
        lease.validate(runtime)?;
        Ok(lease)
    }
    pub(crate) fn home(&self) -> &Path {
        &self.record.home
    }
    pub(crate) fn version(&self) -> &str {
        &self.record.harness_version
    }
    pub(crate) fn native(&self) -> (&Path, &FileStamp) {
        (&self.record.native.path, &self.record.native.stamp)
    }
    pub(crate) fn validate(&self, runtime: &CaptureRuntimeIdentity) -> Result<(), &'static str> {
        if self.record.runtime_core_sha256 != runtime.core_sha256
            || self.record.runtime_guard_sha256 != runtime.guard_sha256
        {
            return Err("RUNTIME_CHANGED");
        }
        if !self.record_stamp.matches(&self.record_path)
            || !self.record_stamp.matches_metadata(
                &self
                    ._record_file
                    .metadata()
                    .map_err(|_| "PROFILE_CHANGED")?,
            )
            || !self.record.home_stamp.matches(&self.record.home)
            || !self.record.profile_stamp.matches(&self.record.profile_root)
            || !self.record.managed_stamp.matches(&self.record.managed_root)
        {
            return Err("PROFILE_CHANGED");
        }
        if !self.record.launcher.matches() || !self.record.native.matches() {
            return Err("HARNESS_CHANGED");
        }
        if !self
            .record
            .generation_stamp
            .matches(&self.record.generation_root)
            || !self.record.files.iter().all(FilePin::matches)
        {
            return Err("REGISTRATION_CHANGED");
        }
        Ok(())
    }
}

pub(crate) fn open_image_metadata(path: &Path) -> Option<File> {
    let file = walk(path, false).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    Some(file)
}
pub(crate) fn image_metadata(path: &Path) -> Option<Metadata> {
    open_image_metadata(path)?.metadata().ok()
}

fn canonical(path: &Path) -> Result<(), AppError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path
            .as_os_str()
            .as_bytes()
            .split(|c| *c == b'/')
            .any(|c| c == b"." || c == b"..")
        || path.canonicalize().map_err(|_| refused())? != path
    {
        return Err(refused());
    }
    Ok(())
}
fn walk(path: &Path, directory: bool) -> Result<File, AppError> {
    if !path.is_absolute()
        || path
            .as_os_str()
            .as_bytes()
            .split(|c| *c == b'/')
            .any(|c| c == b"." || c == b"..")
    {
        return Err(refused());
    }
    let root = CString::new("/").map_err(|_| refused())?;
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(refused());
    }
    let mut parent = unsafe { OwnedFd::from_raw_fd(fd) };
    let parts: Vec<_> = path
        .components()
        .filter_map(|part| {
            if let Component::Normal(value) = part {
                Some(value)
            } else {
                None
            }
        })
        .collect();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(part.as_bytes()).map_err(|_| refused())?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if directory || index + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
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
    }
    Ok(parent.into())
}
fn native_image(bytes: &[u8]) -> bool {
    #[cfg(target_os = "linux")]
    {
        bytes.starts_with(b"\x7fELF")
    }
    #[cfg(target_os = "macos")]
    {
        [
            b"\xcf\xfa\xed\xfe".as_slice(),
            b"\xce\xfa\xed\xfe",
            b"\xfe\xed\xfa\xcf",
            b"\xfe\xed\xfa\xce",
            b"\xca\xfe\xba\xbe",
            b"\xca\xfe\xba\xbf",
        ]
        .iter()
        .any(|magic| bytes.starts_with(magic))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = bytes;
        false
    }
}

fn observed_version(native: &ImagePin, home: &Path, expected: &str) -> Result<(), AppError> {
    let file = walk(&native.path, false)?;
    if !native
        .stamp
        .matches_metadata(&file.metadata().map_err(|_| refused())?)
    {
        return Err(refused());
    }
    let fd = file.as_raw_fd();
    #[cfg(target_os = "linux")]
    let mut command = Command::new(format!("/proc/self/fd/{fd}"));
    #[cfg(not(target_os = "linux"))]
    let mut command = Command::new(&native.path);
    let name = CString::new(native.path.as_os_str().as_bytes()).map_err(|_| refused())?;
    let device = native.stamp.device;
    let inode = native.stamp.inode;
    command
        .env_clear()
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("PATH", "/usr/bin:/bin")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            if libc::lstat(name.as_ptr(), metadata.as_mut_ptr()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let metadata = metadata.assume_init();
            if metadata.st_dev as u128 != u128::from(device)
                || metadata.st_ino as u128 != u128::from(inode)
                || metadata.st_mode & libc::S_IFMT != libc::S_IFREG
            {
                return Err(std::io::Error::from_raw_os_error(libc::ESTALE));
            }
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| refused())?;
    let Some(mut pipe) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(refused());
    };
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(refused());
    }
    let started = Instant::now();
    let mut bytes = Vec::new();
    let status = loop {
        let mut block = [0u8; 1024];
        match pipe.read(&mut block) {
            Ok(n) => bytes.extend_from_slice(&block[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break None,
        }
        if bytes.len() > 8192 || started.elapsed() >= Duration::from_secs(1) {
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if pipe
                    .take(8193 - bytes.len() as u64)
                    .read_to_end(&mut bytes)
                    .is_err()
                {
                    break None;
                }
                break Some(status);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => break None,
        }
    };
    if !status.is_some_and(|status| status.success()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(refused());
    }
    if bytes.len() > 8192
        || !String::from_utf8_lossy(&bytes)
            .split_whitespace()
            .any(|part| part == expected)
        || !native.matches()
    {
        return Err(refused());
    }
    Ok(())
}

pub(crate) struct ProfileOptions<'a> {
    pub directory: &'a Path,
    pub binding: &'a Binding,
    pub namespace: &'a str,
    pub home: &'a Path,
    pub launcher: &'a Path,
    pub native: Option<&'a Path>,
    pub runtime: &'a CaptureRuntimeIdentity,
}

/// Explicit enable-only work: full managed-install inspection, one bounded
/// exact-native version probe and atomic private configuration publication.
/// This returns no transcript capability and cannot bypass normal hook consent.
pub(crate) fn approve(options: ProfileOptions<'_>) -> Result<PreparedProfile, AppError> {
    let (agent, host, source, version) = match options.namespace {
        "claude-code" => (SkillAgent::ClaudeCode, ".claude", "projects", "2.1.319"),
        "codex" => (SkillAgent::Codex, ".codex", "sessions", "0.167.0"),
        _ => return Err(refused()),
    };
    canonical(options.home)?;
    if !crate::skill_adapter::inspect(options.home, agent, Some(options.runtime))? {
        return Err(refused());
    }
    let home_stamp = DirectoryStamp::observe(options.home)?;
    let host_dir = Dir::anchor(options.home)?.child(host, false)?;
    let profile_stamp = DirectoryStamp::observe(&host_dir.path)?;
    let receipt_path = options
        .home
        .join(host)
        .join("skills/keito-time-track/capture-install.json");
    let receipt_stamp = FileStamp::observe(&receipt_path)?;
    let mut receipt_bytes = Vec::new();
    walk(&receipt_path, false)?
        .take(512 * 1024 + 1)
        .read_to_end(&mut receipt_bytes)
        .map_err(|_| refused())?;
    if receipt_bytes.len() > 512 * 1024 {
        return Err(refused());
    }
    let receipt: serde_json::Value =
        serde_json::from_slice(&receipt_bytes).map_err(|_| refused())?;
    if receipt["harness_version"] != version {
        return Err(refused());
    }
    let generation = receipt["generation"]
        .as_str()
        .filter(|generation| hex(generation, 64))
        .ok_or_else(refused)?;
    let generation_root = options
        .home
        .join(host)
        .join(".keito-capture-generations")
        .join(generation);
    let generation_stamp = DirectoryStamp::observe(&generation_root)?;
    let config_path = options.home.join(host).join(if agent == SkillAgent::Codex {
        "hooks.json"
    } else {
        "settings.json"
    });
    let config_stamp = FileStamp::observe(&config_path)?;
    let mut config_bytes = Vec::new();
    walk(&config_path, false)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut config_bytes)
        .map_err(|_| refused())?;
    if config_bytes.len() > 1024 * 1024 {
        return Err(refused());
    }
    let config: serde_json::Value = serde_json::from_slice(&config_bytes).map_err(|_| refused())?;
    let dispatch = generation_root.join("hooks/dispatch.sh");
    let dispatch = dispatch.to_str().ok_or_else(refused)?;
    let mut entries = Vec::new();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "Stop",
        "SessionEnd",
        if agent == SkillAgent::Codex {
            "Interrupt"
        } else {
            "StopFailure"
        },
    ] {
        let groups = config["hooks"][event].as_array().ok_or_else(refused)?;
        let owned: Vec<_> = groups
            .iter()
            .filter(|group| {
                group["hooks"].as_array().is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook["command"]
                            .as_str()
                            .and_then(crate::skill_adapter::registration_words)
                            .is_some_and(|words| words.iter().any(|word| word == dispatch))
                    })
                })
            })
            .collect();
        if owned.len() != 1 {
            return Err(refused());
        }
        entries.push((event, owned[0]));
    }
    let registration_digest = digest(serde_json::to_vec(&entries).map_err(|_| refused())?);
    // An executable option is not a transcript-read escape hatch. Reject both
    // managed source trees before opening either executable candidate.
    for executable in [options.launcher, options.native.unwrap_or(options.launcher)] {
        if [
            options.home.join(".claude/projects"),
            options.home.join(".codex/sessions"),
        ]
        .iter()
        .any(|root| executable.starts_with(root))
        {
            return Err(refused());
        }
    }
    let native_path = options.native.unwrap_or(options.launcher);
    let native = ImagePin::observe(native_path, true)?;
    let launcher = if native_path == options.launcher {
        native.clone()
    } else {
        ImagePin::observe(options.launcher, false)?
    };
    observed_version(&native, options.home, version)?;
    if !receipt_stamp.matches(&receipt_path) || !config_stamp.matches(&config_path) {
        return Err(refused());
    }
    let managed = host_dir.child(source, true)?;
    let managed_stamp = DirectoryStamp::observe(&managed.path)?;
    let mut files = vec![
        FilePin::observe(config_path.clone(), false)?,
        FilePin::observe(receipt_path.clone(), false)?,
        FilePin::observe(generation_root.join("capture-install.json"), false)?,
        FilePin::observe(options.runtime.core_path.clone(), false)?,
        FilePin::observe(options.runtime.guard_path.clone(), false)?,
    ];
    if agent == SkillAgent::Codex {
        files.push(FilePin::observe(
            options.home.join(host).join("config.toml"),
            true,
        )?);
    }
    let payload = receipt["files"].as_object().ok_or_else(refused)?;
    if payload.len() > 64 {
        return Err(refused());
    }
    for relative in payload.keys() {
        let path = Path::new(relative);
        if !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(refused());
        }
        files.push(FilePin::observe(generation_root.join(path), false)?);
    }
    if !crate::skill_adapter::inspect(options.home, agent, Some(options.runtime))?
        || !receipt_stamp.matches(&receipt_path)
        || !config_stamp.matches(&config_path)
        || !files.iter().all(FilePin::matches)
    {
        return Err(refused());
    }
    let id = nonce()?;
    let record = ProfileRecord {
        schema_version: 1,
        profile_id: id.clone(),
        source_namespace: options.namespace.into(),
        harness_version: version.into(),
        home: options.home.into(),
        home_stamp,
        profile_root: host_dir.path.clone(),
        profile_stamp,
        managed_root: managed.path.clone(),
        managed_stamp,
        generation_root,
        generation_stamp,
        registration_digest,
        runtime_core_sha256: options.runtime.core_sha256.clone(),
        runtime_guard_sha256: options.runtime.guard_sha256.clone(),
        launcher,
        native,
        files,
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| refused())?;
    if bytes.len() > MAX_PROFILE_BYTES {
        return Err(refused());
    }
    let profiles = Dir::anchor(options.directory)?.child("profiles", true)?;
    if profiles.metadata()?.mode() & 0o777 != 0o700 {
        return Err(refused());
    }
    profiles.atomic(
        &format!("{id}.json"),
        &bytes,
        0o600,
        Some(None),
        "numeric-profile-published",
    )?;
    let record_path = profiles.path.join(format!("{id}.json"));
    let record_stamp = FileStamp::observe(&record_path)?;
    let prepared = PreparedProfile {
        id,
        digest: digest(bytes),
        binding_id: options.binding.id.clone(),
        binding_version: options.binding.version,
        namespace: options.namespace.into(),
        record,
        record_path,
        record_stamp,
    };
    if !prepared.matches(options.binding, options.namespace) {
        return Err(refused());
    }
    Ok(prepared)
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::fs::PermissionsExt};

    #[test]
    fn enable_time_native_pin_accepts_the_target_claude_image_size_without_execution() {
        // Target compatibility metadata is321,995,200bytes. This owned sparse
        // header fixture tests only enable-time size/hash admission, never a
        // real executable/version, process origin or source approval.
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("owned-inert-large-native");
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        #[cfg(target_os = "macos")]
        file.write_all(b"\xcf\xfa\xed\xfe").unwrap();
        #[cfg(target_os = "linux")]
        file.write_all(b"\x7fELF").unwrap();
        file.set_len(321_995_200).unwrap();
        drop(file);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let pin = ImagePin::observe(&path, true);
        assert!(
            pin.is_ok(),
            "enable-time image bound excludes the supported target's native binary size"
        );
        assert_eq!(pin.unwrap().stamp.bytes, 321_995_200);
    }

    #[test]
    fn enable_time_image_limit_stays_bounded_and_rejects_oversize_metadata() {
        assert_eq!(MAX_IMAGE_BYTES, 512 * 1024 * 1024);
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("owned-inert-oversize");
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.set_len(512 * 1024 * 1024 + 1).unwrap();
        drop(file);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(ImagePin::observe(&path, true).is_err());
    }
}
