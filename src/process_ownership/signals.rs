//! Entry-thread cancellation. Handlers only latch an atomic; they never own a
//! process handle or perform cleanup. No implicit thread-local owner exists.
use std::{io, sync::atomic::{AtomicBool, Ordering}};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupted(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}
pub(crate) fn pending() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}
pub(crate) fn install() -> io::Result<()> {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        action.sa_sigaction = interrupted as *const () as libc::sighandler_t;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}
/// The caller retains the returned Child before restoring this mask. The
/// child's pre_exec callback restores the original mask using libc only.
pub(super) struct AdmissionMask {
    previous: libc::sigset_t,
}
impl AdmissionMask {
    pub(super) fn block(command: &mut std::process::Command) -> io::Result<Self> {
        use std::os::unix::process::CommandExt;
        let mut mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
        let mut previous = unsafe { std::mem::zeroed::<libc::sigset_t>() };
        unsafe {
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGINT);
            libc::sigaddset(&mut mask, libc::SIGTERM);
            if libc::sigprocmask(libc::SIG_BLOCK, &mask, &mut previous) != 0 {
                return Err(io::Error::last_os_error());
            }
            let child_mask = previous;
            command.pre_exec(move || {
                if libc::sigprocmask(libc::SIG_SETMASK, &child_mask, std::ptr::null_mut()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(Self { previous })
    }
}
impl Drop for AdmissionMask {
    fn drop(&mut self) {
        if unsafe { libc::sigprocmask(libc::SIG_SETMASK, &self.previous, std::ptr::null_mut()) } != 0 {
            INTERRUPTED.store(true, Ordering::Relaxed);
        }
    }
}
