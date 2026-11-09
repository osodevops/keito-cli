//! Small, content-blind prelaunch guardian: std, libc and SHA-256 only.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod unix {
    use sha2::{Digest, Sha256};
    use std::{
        ffi::{CStr, CString},
        fs::File,
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt},
        },
        path::{Component, Path, PathBuf},
        process::{Command, Stdio},
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    const PENDING: &[u8] = b"KEITO_CAPTURE_PENDING_V1\n";
    const ACK: &[u8] = b"KEITO_CAPTURE_DURABLE_V1\n";
    static INTERRUPTED: AtomicBool = AtomicBool::new(false);
    extern "C" fn interrupted(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }
    fn c(value: &std::ffi::OsStr) -> Result<CString, ()> {
        CString::new(value.as_bytes()).map_err(|_| ())
    }
    fn root_path() -> Result<PathBuf, ()> {
        if let Some(path) = std::env::var_os("KEITO_CAPTURE_HOME") {
            return Ok(path.into());
        }
        let mut buffer = vec![0u8; 65536];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        if unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        } != 0
            || result.is_null()
        {
            return Err(());
        }
        let entry = unsafe { entry.assume_init() };
        if entry.pw_dir.is_null() {
            return Err(());
        }
        let home = PathBuf::from(std::ffi::OsStr::from_bytes(
            unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes(),
        ));
        #[cfg(target_os = "macos")]
        {
            Ok(home.join("Library/Application Support/keito/capture"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(home.join(".config/keito/capture"))
        }
    }
    fn directory(path: &Path, private_last: usize) -> Result<File, ()> {
        directory_open(path, private_last, true)
    }
    fn directory_open(path: &Path, private_last: usize, create: bool) -> Result<File, ()> {
        if !path.is_absolute()
            || path
                .as_os_str()
                .as_bytes()
                .split(|b| *b == b'/')
                .any(|p| p == b"." || p == b"..")
        {
            return Err(());
        }
        let fd = unsafe {
            libc::open(
                c(Path::new("/").as_os_str())?.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(());
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
        for (i, part) in parts.iter().enumerate() {
            let name = c(part)?;
            let mut fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if create
                && fd < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
            {
                if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(());
                }
                if unsafe { libc::fsync(parent.as_raw_fd()) } != 0 {
                    return Err(());
                }
                fd = unsafe {
                    libc::openat(
                        parent.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
            }
            if fd < 0 {
                return Err(());
            }
            parent = unsafe { OwnedFd::from_raw_fd(fd) };
            let meta = File::from(parent.try_clone().map_err(|_| ())?)
                .metadata()
                .map_err(|_| ())?;
            let uid = unsafe { libc::geteuid() };
            if meta.uid() != 0 && meta.uid() != uid {
                return Err(());
            }
            if i + private_last >= parts.len()
                && (meta.uid() != uid || meta.mode() & 0o777 != 0o700)
            {
                return Err(());
            }
        }
        Ok(parent.into())
    }
    fn executable(path: &Path, expected: &str) -> Result<File, ()> {
        if expected.len() != 64
            || !expected
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(());
        }
        let parent = directory_open(path.parent().ok_or(())?, 0, false)?;
        let name = c(path.file_name().ok_or(())?)?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let before = file.metadata().map_err(|_| ())?;
        if !before.is_file()
            || before.nlink() != 1
            || before.mode() & 0o111 == 0
            || before.mode() & 0o022 != 0
            || before.len() > 128 * 1024 * 1024
            || before.uid() != 0 && before.uid() != unsafe { libc::geteuid() }
        {
            return Err(());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut length = 0;
        loop {
            let n = file.read(&mut buffer).map_err(|_| ())?;
            if n == 0 {
                break;
            }
            length += n;
            if length > 128 * 1024 * 1024 {
                return Err(());
            }
            hash.update(&buffer[..n]);
        }
        let actual: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let after = file.metadata().map_err(|_| ())?;
        if actual != expected
            || before.len() != after.len()
            || before.mode() != after.mode()
            || before.uid() != after.uid()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return Err(());
        }
        Ok(file)
    }
    fn open(parent: &File, name: &str, flags: i32) -> Result<File, ()> {
        let name = CString::new(name).map_err(|_| ())?;
        let started = Instant::now();
        let fd = loop {
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    0o600,
                )
            };
            if fd >= 0 {
                break fd;
            }
            let error = std::io::Error::last_os_error();
            // APFS can report ENOENT for concurrent non-exclusive O_CREAT
            // on this shared lockfile. Retry only that bounded creation path.
            if flags & libc::O_CREAT == 0
                || flags & libc::O_EXCL != 0
                || !matches!(
                    error.raw_os_error(),
                    Some(libc::ENOENT | libc::EINTR | libc::EAGAIN)
                )
                || started.elapsed() >= Duration::from_millis(25)
            {
                return Err(());
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let file = unsafe { File::from_raw_fd(fd) };
        let m = file.metadata().map_err(|_| ())?;
        if !m.is_file()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o777 != 0o600
        {
            return Err(());
        }
        Ok(file)
    }
    fn lock(file: &File) -> bool {
        let start = Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return true;
            }
            if start.elapsed() >= Duration::from_millis(25) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn unlock(file: &File) {
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
        }
    }
    // Rust File::sync_all uses F_FULLFSYNC on Apple. This protocol specifies
    // POSIX fsync, matching the existing capture ledger/adapter contract.
    fn sync(file: &File) -> Result<(), ()> {
        let started = Instant::now();
        loop {
            if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
                return Ok(());
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                || started.elapsed() >= Duration::from_millis(25)
                || INTERRUPTED.load(Ordering::Relaxed)
            {
                return Err(());
            }
        }
    }
    fn set_errno(value: i32) {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = value;
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = value;
        }
    }
    fn errno() -> i32 {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error()
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location()
        }
    }
    fn count_entries(directory: &File, prefix: Option<&[u8]>) -> Result<usize, ()> {
        let fd = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(());
        }
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            unsafe {
                libc::close(fd);
            }
            return Err(());
        }
        let mut count = 0;
        loop {
            set_errno(0);
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let failed = errno() != 0;
                unsafe {
                    libc::closedir(stream);
                }
                return if failed { Err(()) } else { Ok(count) };
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if !matches!(name, b"." | b"..") && prefix.is_none_or(|p| name.starts_with(p)) {
                count += 1;
            }
            if count >= 1024 {
                break;
            }
        }
        unsafe {
            libc::closedir(stream);
        }
        Ok(count)
    }
    fn publish_capacity_refusal(state: &File, nonce: &str) -> Result<(), ()> {
        // Existing refusal saturation already forces a paused core. Never
        // evict another unresolved boundary to make room for this one.
        if count_entries(state, Some(b"capture-refusal-"))? >= 1024 {
            return Err(());
        }
        let temporary = format!(".capture-refusal-{nonce}.tmp");
        let final_name = format!("capture-refusal-{nonce}.json");
        let mut file = open(
            state,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let bytes=format!("{{\"reason\":\"QUEUE_LIMIT_REACHED\",\"boundary_id\":\"{nonce}\",\"observed_at\":{now}}}");
        file.write_all(bytes.as_bytes()).map_err(|_| ())?;
        sync(&file)?;
        if unsafe {
            libc::renameat(
                state.as_raw_fd(),
                CString::new(temporary).unwrap().as_ptr(),
                state.as_raw_fd(),
                CString::new(final_name).unwrap().as_ptr(),
            )
        } != 0
        {
            return Err(());
        }
        sync(state)
    }
    pub fn run() -> Result<(), ()> {
        let started = Instant::now();
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.len() != 8 {
            return Err(());
        }
        let mut core = None;
        let mut agent = None;
        let mut event = None;
        let mut version = None;
        for pair in args.as_chunks::<2>().0 {
            match pair[0].to_str() {
                Some("--core") if core.is_none() => core = Some(PathBuf::from(&pair[1])),
                Some("--agent") if agent.is_none() => agent = pair[1].to_str(),
                Some("--event") if event.is_none() => event = pair[1].to_str(),
                Some("--harness-version") if version.is_none() => version = pair[1].to_str(),
                _ => return Err(()),
            }
        }
        let core = core.ok_or(())?;
        let agent = agent.ok_or(())?;
        let event = event.ok_or(())?;
        let version = version.ok_or(())?;
        if !core.is_absolute()
            || !matches!(agent, "codex" | "claude-code")
            || !matches!(
                event,
                "session-start" | "prompt" | "stop" | "interrupt" | "session-end"
            )
            || version.is_empty()
            || version.len() > 64
            || version.contains("..")
            || !version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            return Err(());
        }
        unsafe {
            libc::signal(
                libc::SIGTERM,
                interrupted as *const () as libc::sighandler_t,
            );
            libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t);
        }
        let root = root_path()?;
        let state = directory(&root, 1).map_err(|_| ())?;
        let flags = directory(&root.join("capture-invocations"), 2).map_err(|_| ())?;
        let publication = open(
            &state,
            "capture-invocations-publication.lock",
            libc::O_RDWR | libc::O_CREAT,
        )
        .map_err(|_| ())?;
        let mut random = [0u8; 32];
        if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
            return Err(());
        }
        let nonce: String = random.iter().map(|b| format!("{b:02x}")).collect();
        if count_entries(&flags, None).map_err(|_| ())? >= 1024 {
            publish_capacity_refusal(&state, &nonce)?;
            return Ok(());
        }
        let preparing = format!("{nonce}.preparing");
        let pending = format!("{nonce}.pending");
        let marker_flags = libc::O_RDWR | libc::O_CREAT | libc::O_EXCL;
        // Darwin can atomically create and lock the visible preparation.
        #[cfg(target_os = "macos")]
        let marker_flags = marker_flags | libc::O_EXLOCK;
        let mut marker = open(&flags, &preparing, marker_flags)?;
        if !lock(&marker) {
            return Err(());
        }
        marker.write_all(PENDING).map_err(|_| ())?;
        sync(&marker)?;
        sync(&flags)?;
        // A live-locked preparing marker is already durable. Keep expensive
        // preparation outside the bounded publication critical section.
        if !lock(&publication) {
            return Ok(());
        }
        if unsafe {
            libc::renameat(
                flags.as_raw_fd(),
                CString::new(preparing).unwrap().as_ptr(),
                flags.as_raw_fd(),
                CString::new(pending.as_str()).unwrap().as_ptr(),
            )
        } != 0
        {
            return Err(());
        }
        sync(&flags)?;
        unlock(&publication);
        let initial = marker.metadata().map_err(|_| ())?;
        if INTERRUPTED.load(Ordering::Relaxed) || started.elapsed() >= Duration::from_millis(600) {
            return Ok(());
        }
        // This digest comes from the fixed managed registration, never stdin.
        // Linux executes the descriptor. Darwin checks its managed pathname
        // immediately before exec and carries this verified imageFD for a
        // synchronous identity cross-check before Tokio/CLI parsing.
        let expected = std::env::var("KEITO_CAPTURE_CORE_SHA256").map_err(|_| ())?;
        let core_file = executable(&core, &expected)?;
        let core_fd = core_file.as_raw_fd();
        if !(4..=1024).contains(&core_fd) {
            return Err(());
        }
        let mut pipe = [-1; 2];
        if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
            return Err(());
        }
        let reader = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
        let writer = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
        for fd in [reader.as_raw_fd(), writer.as_raw_fd()] {
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(());
            }
        }
        if unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
            return Err(());
        }
        let write_fd = writer.as_raw_fd();
        // Inventory this single-threaded process before fork; Command's own
        // later error pipe is CLOEXEC. Never scan or allocate after fork.
        let inventory = unsafe { libc::opendir(CString::new("/dev/fd").unwrap().as_ptr()) };
        if inventory.is_null() {
            return Err(());
        }
        let inventory_fd = unsafe { libc::dirfd(inventory) };
        let mut inherited = Vec::new();
        loop {
            set_errno(0);
            let entry = unsafe { libc::readdir(inventory) };
            if entry.is_null() {
                if errno() != 0 {
                    unsafe {
                        libc::closedir(inventory);
                    }
                    return Err(());
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if let Some(fd) = name.to_str().ok().and_then(|name| name.parse::<i32>().ok()) {
                if fd >= 4 && fd != inventory_fd {
                    inherited.push(fd);
                }
            }
        }
        unsafe {
            libc::closedir(inventory);
        }
        #[cfg(target_os = "macos")]
        let mut command = Command::new(&core);
        #[cfg(not(target_os = "macos"))]
        let mut command = Command::new(format!("/dev/fd/{core_fd}"));
        let core_name = c(core.as_os_str())?;
        let core_identity = core_file.metadata().map_err(|_| ())?;
        command
            .args([
                "capture",
                "hook",
                "--agent",
                agent,
                "--event",
                event,
                "--harness-version",
                version,
            ])
            .env("KEITO_CAPTURE_INVOCATION_NONCE", nonce)
            .env("KEITO_CAPTURE_ACK_FD", "3")
            .env("KEITO_CAPTURE_IMAGE_FD", core_fd.to_string())
            .stdin(Stdio::inherit())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                // Darwin has no public exec-by-fd. Verify the fixed managed
                // path immediately before exec; synchronous core bootstrap
                // cross-checks its actual executable inode against imageFD.
                let mut current = std::mem::MaybeUninit::<libc::stat>::uninit();
                if libc::lstat(core_name.as_ptr(), current.as_mut_ptr()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let current = current.assume_init();
                if current.st_dev as u128 != u128::from(core_identity.dev())
                    || u128::from(current.st_ino) != u128::from(core_identity.ino())
                    || current.st_mode & libc::S_IFMT != libc::S_IFREG
                {
                    return Err(std::io::Error::from_raw_os_error(libc::ESTALE));
                }
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::dup2(write_fd, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(core_fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for fd in &inherited {
                    if *fd >= 4 && *fd != core_fd {
                        libc::close(*fd);
                    }
                }
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|_| ())?;
        drop(writer);
        let mut reader = File::from(reader);
        let mut bytes = Vec::with_capacity(ACK.len() + 1);
        let mut eof = false;
        let mut status = None;
        loop {
            let mut part = [0u8; 64];
            match reader.read(&mut part) {
                Ok(0) => eof = true,
                Ok(n) => {
                    bytes.extend_from_slice(&part[..n]);
                    if bytes.len() > ACK.len() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
            status = child.try_wait().map_err(|_| ())?;
            if status.is_some() && eof {
                break;
            }
            if INTERRUPTED.load(Ordering::Relaxed)
                || started.elapsed() >= Duration::from_millis(600)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if status.is_none() {
            let pid = child.id() as i32;
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            let end = Instant::now();
            loop {
                if child.try_wait().map_err(|_| ())?.is_some() {
                    break;
                }
                if end.elapsed() >= Duration::from_millis(15) {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if status.is_some_and(|s| s.success())
            && eof
            && bytes == ACK
            && !INTERRUPTED.load(Ordering::Relaxed)
            && started.elapsed() < Duration::from_millis(600)
            && lock(&publication)
        {
            if let Ok(current) = open(&flags, &pending, libc::O_RDONLY) {
                let m = current.metadata().map_err(|_| ())?;
                let mut content = Vec::new();
                current
                    .take((PENDING.len() + 1) as u64)
                    .read_to_end(&mut content)
                    .map_err(|_| ())?;
                if m.dev() == initial.dev()
                    && m.ino() == initial.ino()
                    && m.len() == PENDING.len() as u64
                    && content == PENDING
                    && unsafe {
                        libc::unlinkat(
                            flags.as_raw_fd(),
                            CString::new(pending).unwrap().as_ptr(),
                            0,
                        )
                    } == 0
                {
                    sync(&flags)?;
                }
            }
            unlock(&publication);
        }
        Ok(())
    }
}
pub fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let _ = unix::run();
}
