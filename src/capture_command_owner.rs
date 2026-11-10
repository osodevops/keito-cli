//! Command ownership for actual capture probes. No credentials, consent or
//! qualified-source authority is constructed here.
use crate::{
    capture_runtime_pair,
    error::AppError,
    process_ownership::{
        image::VerifiedKeeperImage,
        io::IoPlan,
        probes::LeafRunner,
        protocol::ProtocolError,
        scope::{CommandResult, Scope},
    },
};
use std::{io, process::Command};

const STANDALONE_NS: u64 = 1_000_000_000;
const HOOK_NS: u64 = 500_000_000;
pub(crate) const LEAF_END_ENV: &str = "KEITO_CAPTURE_LEAF_END_NS";
fn refused() -> AppError {
    AppError::Config("Capture command ownership unavailable; collection remains paused".into())
}
const PARENT_KEYS: [&str; 7] = [
    "KEITO_PROCESS_OWNER_FD",
    "KEITO_PROCESS_OWNER_NONCE",
    "KEITO_PROCESS_OWNER_PID",
    "KEITO_PROCESS_OWNER_END_NS",
    "KEITO_PROCESS_OWNER_CONTRACT",
    "KEITO_SOURCE_OWNER_FD",
    "KEITO_SOURCE_OWNER_NONCE",
];
fn parent_present(mut present: impl FnMut(&str) -> bool) -> bool {
    PARENT_KEYS.iter().any(|key| present(key))
}
enum Image<'a> {
    Owned(VerifiedKeeperImage),
    Borrowed(&'a VerifiedKeeperImage),
}
pub(crate) struct CommandOwner<'a> {
    scope: ScopeRef<'a>,
    image: Image<'a>,
    end: u64,
}
enum ScopeRef<'a> { Owned(Scope), Borrowed(&'a mut Scope) }
impl std::ops::Deref for ScopeRef<'_> {
    type Target = Scope;
    fn deref(&self) -> &Scope { match self { Self::Owned(s) => s, Self::Borrowed(s) => s } }
}
impl std::ops::DerefMut for ScopeRef<'_> {
    fn deref_mut(&mut self) -> &mut Scope { match self { Self::Owned(s) => s, Self::Borrowed(s) => s } }
}
impl CommandOwner<'static> {
    pub(crate) fn standalone() -> Result<Self, AppError> {
        // Never ignore an attached/partial parent in a convenience API. The
        // actual entry Session must pass its borrowed runner explicitly.
        if parent_present(|key| std::env::var_os(key).is_some()) {
            return Err(refused());
        }
        for key in [LEAF_END_ENV, "KEITO_CAPTURE_INVOCATION_NONCE"] {
            if std::env::var_os(key).is_some() {
                return Err(refused());
            }
        }
        let end = crate::process_ownership::scope::monotonic_now()
            .map_err(|_| refused())?
            .saturating_add(STANDALONE_NS);
        let identity = capture_runtime_pair::current()?;
        let image = VerifiedKeeperImage::open(&identity.guard_path, &identity.guard_sha256)
            .map_err(|_| refused())?;
        Ok(Self {
            scope: ScopeRef::Owned(Scope::with_owner(Default::default(), end)),
            image: Image::Owned(image),
            end,
        })
    }
}
impl<'a> CommandOwner<'a> {
    pub(crate) fn for_hook(
        runtime: &'a capture_runtime_pair::HookRuntime,
        scope: &'a mut Scope,
    ) -> Result<Self, AppError> {
        let at = crate::process_ownership::scope::monotonic_now().map_err(|_| refused())?;
        let raw = std::env::var(LEAF_END_ENV).map_err(|_| refused())?;
        let inherited = raw
            .parse::<u64>()
            .ok()
            .filter(|n| n.to_string() == raw && *n > at)
            .ok_or_else(refused)?;
        let end = inherited.min(at.saturating_add(HOOK_NS)).min(scope.hard_end());
        scope.tighten_end(end);
        Ok(Self {
            scope: ScopeRef::Borrowed(scope),
            image: Image::Borrowed(runtime.keeper()),
            end,
        })
    }
    pub(crate) fn finish_clean(&mut self) -> bool {
        self.scope.finish().clean
    }
    pub(crate) fn finish<T>(&mut self, result: Result<T, AppError>) -> Result<T, AppError> {
        if !self.finish_clean() {
            return Err(refused());
        }
        result
    }
    pub(crate) fn fail_ack(&mut self) {
        self.scope
            .begin_cancel(ProtocolError("HOOK_ACK_WRITE"), self.end);
    }
    pub(crate) fn read_event(&mut self, max: usize) -> Result<Vec<u8>, AppError> {
        struct Flags(i32, crate::process_ownership::protocol::SharedOwner);
        impl Drop for Flags {
            fn drop(&mut self) {
                if unsafe { libc::fcntl(0, libc::F_SETFL, self.0) } < 0 {
                    self.1
                        .borrow_mut()
                        .poison(ProtocolError("HOOK_INPUT_FLAGS"));
                }
            }
        }
        let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            self.scope
                .begin_cancel(ProtocolError("HOOK_INPUT_FLAGS"), self.end);
            return Err(refused());
        }
        let _flags = Flags(flags, self.scope.owner_state());
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(max.saturating_add(1)).is_err()
            || bytes.capacity() > max.saturating_add(1)
        {
            self.scope
                .begin_cancel(ProtocolError("HOOK_INPUT_ALLOCATION"), self.end);
            return Err(refused());
        }
        loop {
            self.scope.poll();
            if self.scope.first_fault().is_some() {
                return Err(refused());
            }
            let mut part = [0u8; 4096];
            let count = unsafe { libc::read(0, part.as_mut_ptr().cast(), part.len()) };
            if count == 0 {
                return Ok(bytes);
            }
            if count > 0 {
                let count = count as usize;
                if bytes.len().saturating_add(count) > max {
                    self.scope
                        .begin_cancel(ProtocolError("HOOK_INPUT_LIMIT"), self.end);
                    return Err(refused());
                }
                bytes.extend_from_slice(&part[..count]);
            } else {
                let e = io::Error::last_os_error();
                if !matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) {
                    self.scope
                        .begin_cancel(ProtocolError("HOOK_INPUT_READ"), self.end);
                    return Err(refused());
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}
impl LeafRunner for CommandOwner<'_> {
    fn output(
        &mut self,
        command: Command,
        stdout_cap: usize,
        stderr_cap: usize,
    ) -> io::Result<CommandResult> {
        let image = match &self.image {
            Image::Owned(image) => image,
            Image::Borrowed(image) => image,
        };
        self.scope.run_verified(
            command,
            IoPlan::new(Vec::new(), stdout_cap, stderr_cap)?,
            image,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn r40_any_partial_parent_requires_the_explicit_entry_owner() {
        assert!(!parent_present(|_| false));
        for field in PARENT_KEYS {
            assert!(parent_present(|key| key == field));
        }
    }
    #[test]
    fn r41_installed_owner_requires_the_new_guard_contract() {
        assert_eq!(crate::capture_runtime_pair::GUARD_CONTRACT, 2);
    }
    #[test]
    fn r41_dispatch_execs_the_native_owner_without_watchdog_children() {
        let source = include_str!("../skills/keito-time-track/hooks/dispatch.sh");
        assert!(source.contains("exec \"$guard_cmd\" --internal-dispatch-v2"));
        assert!(!source.contains("sleep "));
        assert!(!source.contains("watchdog="));
        assert!(!source.contains("kill -"));
    }
}
