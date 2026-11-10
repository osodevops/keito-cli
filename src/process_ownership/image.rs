//! Retained guard-image identity for internal resource ownership only.
//! A private Rust constructor is not an OS file seal: snapshot/exec races remain
//! subject to the same inode checks as the existing core bootstrap.
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Read},
    os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt}},
    path::{Component, Path, PathBuf},
    process::Command,
};

const MAX_IMAGE: u64 = 128 * 1024 * 1024;
const IMAGE_ENV: &str = "KEITO_CAPTURE_KEEPER_IMAGE_FD";
pub(crate) const KEEPER_ROLE: &str = "--internal-keeper-v1";
fn refused() -> io::Error { io::Error::other("KEEPER_IMAGE") }

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    device: u64, inode: u64, size: u64, mode: u32, uid: u32, links: u64,
    modified: i64, modified_ns: i64, changed: i64, changed_ns: i64,
}
impl Stamp {
    fn of(m: &fs::Metadata) -> Self {
        Self { device: m.dev(), inode: m.ino(), size: m.len(), mode: m.mode(), uid: m.uid(), links: m.nlink(),
            modified: m.mtime(), modified_ns: m.mtime_nsec(), changed: m.ctime(), changed_ns: m.ctime_nsec() }
    }
}

/// The only product caller is the compiled-anchor HookRuntime factory. Public
/// runtime descriptors and caller-supplied manifests cannot construct this type.
pub(crate) struct VerifiedKeeperImage { file: File, path: PathBuf, stamp: Stamp }
impl VerifiedKeeperImage {
    pub(crate) fn open(path: &Path, expected: &str) -> io::Result<Self> {
        if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !path.is_absolute() || path.components().any(|p| matches!(p, Component::ParentDir | Component::CurDir)) {
            return Err(refused());
        }
        let root = CString::new("/").unwrap();
        let fd = unsafe { libc::open(root.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if fd < 0 { return Err(refused()); }
        let mut parent = unsafe { OwnedFd::from_raw_fd(fd) };
        let parts: Vec<_> = path.components().filter_map(|p| if let Component::Normal(p)=p {Some(p)} else {None}).collect();
        let (leaf, ancestors) = parts.split_last().ok_or_else(refused)?;
        for part in ancestors {
            let name = CString::new(part.as_bytes()).map_err(|_| refused())?;
            let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
            if fd < 0 { return Err(refused()); }
            parent = unsafe { OwnedFd::from_raw_fd(fd) };
        }
        let name = CString::new(leaf.as_bytes()).map_err(|_| refused())?;
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 { return Err(refused()); }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let before = file.metadata().map_err(|_| refused())?;
        // Separate product policy; the legacy QA constructor still requires700.
        if !before.is_file() || before.mode() & 0o111 == 0 || before.mode() & 0o022 != 0 || before.nlink() != 1
            || (before.uid() != 0 && before.uid() != unsafe { libc::geteuid() }) || before.len() > MAX_IMAGE {
            return Err(refused());
        }
        let stamp = Stamp::of(&before);
        let mut digest = Sha256::new();
        let mut bytes = [0u8; 65536];
        let mut length = 0u64;
        loop {
            let n = file.read(&mut bytes).map_err(|_| refused())?;
            if n == 0 { break; }
            length = length.checked_add(n as u64).filter(|n| *n <= MAX_IMAGE).ok_or_else(refused)?;
            digest.update(&bytes[..n]);
        }
        let actual: String = digest.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if actual != expected || length != before.len() || Stamp::of(&file.metadata().map_err(|_| refused())?) != stamp {
            return Err(refused());
        }
        let result = Self { file, path: path.into(), stamp };
        result.check_current()?;
        Ok(result)
    }
    fn check_current(&self) -> io::Result<()> {
        let file = self.file.metadata().map_err(|_| refused())?;
        let path = fs::symlink_metadata(&self.path).map_err(|_| refused())?;
        if Stamp::of(&file) != self.stamp || Stamp::of(&path) != self.stamp || path.file_type().is_symlink() {
            return Err(refused());
        }
        Ok(())
    }
    // Keep the duplicated image FD alive in the parent until spawn has returned
    // its real Child. All descriptor inventory/allocation occurs before fork.
    pub(super) fn command(&self, control: i32) -> io::Result<(Command, OwnedFd)> {
        if !(4..=1024).contains(&control) {return Err(refused());}
        self.check_current()?;
        let fd = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 5) };
        if fd < 0 { return Err(refused()); }
        let image = unsafe { OwnedFd::from_raw_fd(fd) };
        if fd>1024 || fd==control {return Err(refused());}
        let inherited = inventory(control, fd)?;
        let path = CString::new(self.path.as_os_str().as_bytes()).map_err(|_| refused())?;
        let stamp = self.stamp;
        #[cfg(target_os="linux")]
        let mut command = Command::new(format!("/dev/fd/{fd}"));
        #[cfg(not(target_os="linux"))]
        let mut command = Command::new(&self.path);
        command.arg(KEEPER_ROLE);
        unsafe {
            command.pre_exec(move || {
                let mut current = std::mem::MaybeUninit::<libc::stat>::uninit();
                if libc::lstat(path.as_ptr(), current.as_mut_ptr()) != 0 { return Err(io::Error::last_os_error()); }
                let current = current.assume_init();
                if current.st_dev as u128 != u128::from(stamp.device) || u128::from(current.st_ino) != u128::from(stamp.inode)
                    || current.st_mode & libc::S_IFMT != libc::S_IFREG {
                    return Err(io::Error::from_raw_os_error(libc::ESTALE));
                }
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 { return Err(io::Error::last_os_error()); }
                for other in &inherited { libc::close(*other); }
                Ok(())
            });
        }
        Ok((command, image))
    }
    pub(super) fn image_env() -> &'static str { IMAGE_ENV }
}

#[cfg(any(target_os="macos",target_os="linux"))]
fn inventory(control: i32, image: i32) -> io::Result<Vec<i32>> {
    use std::ffi::CStr;
    let directory = CString::new("/dev/fd").unwrap();
    let raw = unsafe { libc::opendir(directory.as_ptr()) };
    if raw.is_null() { return Err(refused()); }
    let own = unsafe { libc::dirfd(raw) };
    let mut descriptors = Vec::new();
    let failed = loop {
        #[cfg(target_os="macos")]
        unsafe { *libc::__error()=0; }
        #[cfg(target_os="linux")]
        unsafe { *libc::__errno_location()=0; }
        let entry = unsafe { libc::readdir(raw) };
        if entry.is_null() { break io::Error::last_os_error().raw_os_error().unwrap_or(0) != 0; }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if let Some(fd) = name.to_str().ok().and_then(|n| n.parse::<i32>().ok()) {
            if fd >= 3 && fd != own && fd != control && fd != image { descriptors.push(fd); }
        }
    };
    unsafe { libc::closedir(raw); }
    if failed { Err(refused()) } else { Ok(descriptors) }
}
#[cfg(not(any(target_os="macos",target_os="linux")))]
fn inventory(_:i32,_:i32)->io::Result<Vec<i32>> { Err(refused()) }

/// Private keeper role entry, before parsing keeper controls or touching capture
/// state. The descriptor is consumed/closed, never turned into capture authority.
#[allow(dead_code)] // Called by the standalone guard's inclusion, not by libkeito.
pub(crate) fn bootstrap_keeper_image(control: i32) -> io::Result<()> {
    let raw = std::env::var(IMAGE_ENV).map_err(|_| refused())?;
    std::env::remove_var(IMAGE_ENV);
    let fd = bootstrap_fd(&raw,control)?;
    let file = unsafe { File::from_raw_fd(fd) };
    if unsafe { libc::fcntl(fd,libc::F_SETFD,libc::FD_CLOEXEC) } < 0 { return Err(refused()); }
    let metadata = file.metadata().map_err(|_| refused())?;
    let running = fs::metadata(std::env::current_exe().map_err(|_| refused())?).map_err(|_| refused())?;
    if !metadata.is_file() || metadata.len()>MAX_IMAGE || metadata.nlink()!=1 || metadata.mode() & 0o022 != 0 || metadata.mode() & 0o111 == 0
        || (metadata.uid()!=0 && metadata.uid()!=unsafe {libc::geteuid()})
        || unsafe {libc::fcntl(fd,libc::F_GETFL)} & libc::O_ACCMODE != libc::O_RDONLY
        || metadata.dev()!=running.dev() || metadata.ino()!=running.ino() { return Err(refused()); }
    Ok(())
}
fn bootstrap_fd(raw:&str,control:i32)->io::Result<i32> {
    raw.parse::<i32>().ok().filter(|fd|(5..=1024).contains(fd) && *fd!=control && fd.to_string()==raw).ok_or_else(refused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt,symlink};
    struct Inert { _temp: tempfile::TempDir, path: PathBuf, hash: String }
    impl Inert {
        fn new(mode:u32)->Self {
            let temp=tempfile::tempdir().unwrap();let path=temp.path().canonicalize().unwrap().join("keito-capture-guard");
            fs::write(&path,b"inert guard bytes; MUST NEVER EXECUTE").unwrap();
            fs::set_permissions(&path,fs::Permissions::from_mode(mode)).unwrap();
            let hash=Sha256::digest(fs::read(&path).unwrap()).iter().map(|b|format!("{b:02x}")).collect();
            Self {_temp:temp,path,hash}
        }
    }
    #[test]
    fn r40_official_755_guard_has_distinct_verified_image_admission() {
        let f=Inert::new(0o755);let image=VerifiedKeeperImage::open(&f.path,&f.hash).unwrap();
        assert!(image.check_current().is_ok());
        assert_eq!(unsafe {libc::fcntl(image.file.as_raw_fd(),libc::F_GETFD)} & libc::FD_CLOEXEC,libc::FD_CLOEXEC);
    }
    #[test]
    fn r40_unsigned_wrong_hash_writable_and_symlink_images_refuse() {
        let f=Inert::new(0o700);assert!(VerifiedKeeperImage::open(&f.path,&"0".repeat(64)).is_err());
        assert!(VerifiedKeeperImage::open(&f.path,"").is_err());
        for mode in [0o600,0o777] {
            fs::set_permissions(&f.path,fs::Permissions::from_mode(mode)).unwrap();
            assert!(VerifiedKeeperImage::open(&f.path,&f.hash).is_err());
        }
        let f=Inert::new(0o700);let link=f.path.with_extension("link");symlink(&f.path,&link).unwrap();
        assert!(VerifiedKeeperImage::open(&link,&f.hash).is_err());
    }
    #[test]
    fn r40_retained_descriptor_detects_replacement_edit_and_hardlink() {
        for change in ["replace","edit","link"] {
            let f=Inert::new(0o700);let image=VerifiedKeeperImage::open(&f.path,&f.hash).unwrap();
            match change {
                "replace"=>{fs::rename(&f.path,f.path.with_extension("old")).unwrap();fs::write(&f.path,b"replacement").unwrap();},
                "edit"=>fs::write(&f.path,b"different size").unwrap(),
                _=>fs::hard_link(&f.path,f.path.with_extension("link")).unwrap(),
            }
            assert!(image.check_current().is_err());
        }
        let f=Inert::new(0o700);fs::hard_link(&f.path,f.path.with_extension("link")).unwrap();
        assert!(VerifiedKeeperImage::open(&f.path,&f.hash).is_err());
    }
    #[test]
    fn r40_image_bootstrap_fd_is_canonical_and_never_the_control_socket() {
        assert_eq!(bootstrap_fd("5",4).unwrap(),5);
        for raw in ["0","3","4","05","+5"," 5","1025","-1",""] {assert!(bootstrap_fd(raw,4).is_err());}
        assert!(bootstrap_fd("5",5).is_err());
    }
    #[test]
    fn r40_image_parent_replaced_with_symlink_is_not_a_live_managed_path() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
        let managed=root.join("managed");fs::create_dir(&managed).unwrap();
        let path=managed.join("keito-capture-guard");fs::write(&path,b"inert; never execute").unwrap();
        fs::set_permissions(&path,fs::Permissions::from_mode(0o700)).unwrap();
        let hash:String=Sha256::digest(fs::read(&path).unwrap()).iter().map(|b|format!("{b:02x}")).collect();
        let image=VerifiedKeeperImage::open(&path,&hash).unwrap();
        let old=root.join("old");fs::rename(&managed,&old).unwrap();symlink(&old,&managed).unwrap();
        assert!(image.check_current().is_err());
    }
}
