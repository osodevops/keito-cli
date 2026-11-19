//! Bounded cooperative native-image provenance, never provider attestation.
//! Inspect at most eight same-user ancestors, only PID/start/parent/image
//! metadata. No argv, environment, cwd, open-file list or process enumeration.
#![cfg(unix)]
use crate::capture_usage_profile::FileStamp;
use std::{
    collections::BTreeSet,
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq, Eq)]
struct Snapshot {
    pid: i32,
    parent: i32,
    started: (u64, u64),
    image: PathBuf,
}
pub(crate) struct OriginLease {
    nodes: Vec<Snapshot>,
    native_path: PathBuf,
    native_stamp: FileStamp,
    native_file: File,
}

impl OriginLease {
    pub(crate) fn verify(native_path: &Path, native_stamp: &FileStamp) -> Option<Self> {
        #[cfg(test)]
        crate::capture_usage_test_io::observe(1);
        let (nodes, native_file) =
            collect_chain(unsafe { libc::getppid() }, observe, |node, file| {
                node.image == native_path
                    && file
                        .metadata()
                        .is_ok_and(|metadata| native_stamp.matches_metadata(&metadata))
            })?;
        let lease = Self {
            nodes,
            native_path: native_path.into(),
            native_stamp: native_stamp.clone(),
            native_file,
        };
        lease.valid().then_some(lease)
    }
    pub(crate) fn valid(&self) -> bool {
        if self.nodes.is_empty()
            || self.nodes.len() > 8
            || unsafe { libc::getppid() } != self.nodes[0].pid
            || !self
                .native_stamp
                .matches_metadata(&match self.native_file.metadata() {
                    Ok(metadata) => metadata,
                    Err(_) => return false,
                })
        {
            return false;
        }
        if !stable_chain(&self.nodes, unsafe { libc::getppid() }, observe) {
            return false;
        }
        let Some(native) = self.nodes.last() else {
            return false;
        };
        if native.image != self.native_path {
            return false;
        }
        // Current named-image freshness is checked as well as the executing
        // image. Replaced/upgraded binaries need explicit profile approval.
        crate::capture_usage_profile::image_metadata(&self.native_path)
            .is_some_and(|metadata| self.native_stamp.matches_metadata(&metadata))
    }
}

// Both helpers consume only the fixed metadata observations. Tests provide inert
// snapshots, never an alternate positive runtime or origin capability.
fn collect_chain(
    mut pid: i32,
    mut read: impl FnMut(i32) -> Option<(Snapshot, File)>,
    matches: impl Fn(&Snapshot, &File) -> bool,
) -> Option<(Vec<Snapshot>, File)> {
    let mut seen = BTreeSet::new();
    let mut nodes = Vec::new();
    for _ in 0..8 {
        if pid <= 0 || !seen.insert(pid) {
            return None;
        }
        let (node, file) = read(pid)?;
        if node.pid != pid || node.started.0 == 0 || node.parent == pid {
            return None;
        }
        let matched = matches(&node, &file);
        pid = node.parent;
        nodes.push(node);
        if matched {
            return Some((nodes, file));
        }
    }
    None
}
fn stable_chain(
    nodes: &[Snapshot],
    parent: i32,
    mut read: impl FnMut(i32) -> Option<(Snapshot, File)>,
) -> bool {
    !nodes.is_empty()
        && nodes.len() <= 8
        && nodes[0].pid == parent
        && nodes.windows(2).all(|pair| pair[0].parent == pair[1].pid)
        && nodes
            .iter()
            .all(|node| read(node.pid).is_some_and(|(current, _)| current == *node))
}

#[cfg(target_os = "macos")]
fn observe(pid: i32) -> Option<(Snapshot, File)> {
    use std::os::unix::ffi::OsStringExt;
    fn identity(pid: i32) -> Option<(i32, (u64, u64))> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
        let length = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if length != size {
            return None;
        }
        let info = unsafe { info.assume_init() };
        if info.pbi_pid != u32::try_from(pid).ok()?
            || info.pbi_uid != unsafe { libc::geteuid() }
            || info.pbi_ruid != unsafe { libc::geteuid() }
            || info.pbi_start_tvsec == 0
        {
            return None;
        }
        Some((
            i32::try_from(info.pbi_ppid).ok()?,
            (info.pbi_start_tvsec, info.pbi_start_tvusec),
        ))
    }
    let before = identity(pid)?;
    let mut bytes = [0u8; 4096];
    let length = unsafe {
        libc::proc_pidpath(
            pid,
            bytes.as_mut_ptr().cast(),
            u32::try_from(bytes.len()).ok()?,
        )
    };
    if length <= 0 || usize::try_from(length).ok()? >= bytes.len() {
        return None;
    }
    let end = bytes.iter().position(|byte| *byte == 0)?;
    let image = PathBuf::from(std::ffi::OsString::from_vec(bytes[..end].to_vec()));
    if !image.is_absolute() || identity(pid)? != before {
        return None;
    }
    // Darwin reports a kernel path rather than an executable descriptor. Open
    // it through the same no-follow metadata boundary and repeat PID/start/path
    // observations in valid(). This retains the documented Darwin path/image
    // race and cooperative threat limit; it is not executable sealing.
    let file = crate::capture_usage_profile::open_image_metadata(&image)?;
    Some((
        Snapshot {
            pid,
            parent: before.0,
            started: before.1,
            image,
        },
        file,
    ))
}

#[cfg(target_os = "linux")]
fn observe(pid: i32) -> Option<(Snapshot, File)> {
    use std::{io::Read, os::unix::fs::MetadataExt};
    fn identity(pid: i32) -> Option<(i32, (u64, u64))> {
        if pid <= 0
            || std::fs::metadata(format!("/proc/{pid}")).ok()?.uid() != unsafe { libc::geteuid() }
        {
            return None;
        }
        let mut status = Vec::new();
        File::open(format!("/proc/{pid}/status"))
            .ok()?
            .take(65537)
            .read_to_end(&mut status)
            .ok()?;
        if status.len() > 65536 {
            return None;
        }
        let status = std::str::from_utf8(&status).ok()?;
        let uids: Vec<u32> = status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))?
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        if uids.len() != 4 || uids.iter().any(|uid| *uid != unsafe { libc::geteuid() }) {
            return None;
        }
        let mut data = Vec::new();
        File::open(format!("/proc/{pid}/stat"))
            .ok()?
            .take(4097)
            .read_to_end(&mut data)
            .ok()?;
        if data.len() > 4096 {
            return None;
        }
        let text = std::str::from_utf8(&data).ok()?;
        let prefix = text.find(' ')?;
        if text[..prefix].parse::<i32>().ok()? != pid {
            return None;
        }
        let end = text.rfind(')')?;
        let fields: Vec<_> = text[end + 1..].split_whitespace().take(20).collect();
        if fields.len() != 20 {
            return None;
        }
        Some((fields[1].parse().ok()?, (fields[19].parse().ok()?, 0)))
    }
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c"/proc".as_ptr(), filesystem.as_mut_ptr()) } != 0 {
        return None;
    }
    if i128::from(unsafe { filesystem.assume_init() }.f_type) != i128::from(libc::PROC_SUPER_MAGIC)
    {
        return None;
    }
    let before = identity(pid)?;
    let image = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    if !image.is_absolute() {
        return None;
    }
    // The kernel procfs executable link intentionally follows the executing
    // image, unlike configured paths. Opening it pins that inode for the lease.
    let file = File::open(format!("/proc/{pid}/exe")).ok()?;
    if identity(pid)? != before {
        return None;
    }
    Some((
        Snapshot {
            pid,
            parent: before.0,
            started: before.1,
            image,
        },
        file,
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn observe(_: i32) -> Option<(Snapshot, File)> {
    None
}

#[cfg(test)]
#[path = "../tests/unit/capture_usage_origin.rs"]
mod tests;
