//! Command ownership for actual capture probes. No credentials, consent or
//! qualified-source authority is constructed here.
use crate::{
    capture_runtime_pair,
    error::{AppError, CaptureProbeReason},
    process_ownership::{
        group::CleanupPolicy,
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
// This is the command-owner lifetime, not a whole-install/whole-usage wall-clock
// guarantee: some high-level callers verify a pair before constructing it.
fn probe_policy(hook: bool) -> CleanupPolicy {
    if hook {
        CleanupPolicy::HookShort
    } else {
        CleanupPolicy::CommandProbe
    }
}
pub(crate) const LEAF_END_ENV: &str = "KEITO_CAPTURE_LEAF_END_NS";
fn refused() -> AppError {
    AppError::Config("Capture command ownership unavailable; collection remains paused".into())
}
pub(crate) fn version_probe_error(error: io::Error) -> AppError {
    let reason = match error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<ProtocolError>())
    {
        Some(ProtocolError("SCOPE_ADMISSION")) => CaptureProbeReason::AdmissionRefused,
        Some(ProtocolError(
            "SCOPE_OPERATION_DEADLINE"
            | "SCOPE_PROOF_DEADLINE"
            | "SCOPE_CLEANUP_DEADLINE"
            | "GROUP_DEADLINE"
            | "GROUP_LIFETIME",
        )) => CaptureProbeReason::Deadline,
        Some(ProtocolError("SCOPE_CLOCK")) => CaptureProbeReason::Clock,
        Some(_) => CaptureProbeReason::Ownership,
        None => CaptureProbeReason::Unavailable,
    };
    AppError::CaptureVersionProbe {
        reason,
        cleanup_unverified: false,
    }
}
fn finish_result<T>(result: Result<T, AppError>, clean: bool) -> Result<T, AppError> {
    if clean {
        return result;
    }
    match result {
        Err(AppError::CaptureVersionProbe { reason, .. }) => Err(AppError::CaptureVersionProbe {
            reason,
            cleanup_unverified: true,
        }),
        _ => Err(refused()),
    }
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
    policy: CleanupPolicy,
}
enum ScopeRef<'a> {
    Owned(Box<Scope>),
    Borrowed(&'a mut Scope),
}
impl std::ops::Deref for ScopeRef<'_> {
    type Target = Scope;
    fn deref(&self) -> &Scope {
        match self {
            Self::Owned(s) => s,
            Self::Borrowed(s) => s,
        }
    }
}
impl std::ops::DerefMut for ScopeRef<'_> {
    fn deref_mut(&mut self) -> &mut Scope {
        match self {
            Self::Owned(s) => s,
            Self::Borrowed(s) => s,
        }
    }
}
fn local_install_scope(defaults: bool) -> Result<Scope, AppError> {
    if parent_present(|key| std::env::var_os(key).is_some())
        || [LEAF_END_ENV, "KEITO_CAPTURE_INVOCATION_NONCE"]
            .iter()
            .any(|key| std::env::var_os(key).is_some())
    {
        return Err(refused());
    }
    let at = crate::process_ownership::scope::monotonic_now().map_err(|_| refused())?;
    let end = at
        .checked_add(if defaults {
            STANDALONE_NS
        } else {
            crate::process_ownership::scope::INSTALL_OPERATION_NS
        })
        .ok_or_else(refused)?;
    Scope::local_entry(end).map_err(|_| refused())
}
pub(crate) fn defaults_install_scope() -> Result<Scope, AppError> {
    local_install_scope(true)
}

/// One consuming preflight on the containing operation; no second endpoint.
pub(crate) struct InstallerPreflight<'a> {
    scope: Option<ScopeRef<'a>>,
    image: Option<VerifiedKeeperImage>,
    runtime: Option<capture_runtime_pair::CaptureRuntimeIdentity>,
}
pub(crate) struct InstallerPublication<'a> {
    scope: ScopeRef<'a>,
    finished: bool,
}
impl<'a> InstallerPreflight<'a> {
    pub(crate) fn new(scope: Option<&'a mut Scope>, expected: usize) -> Result<Self, AppError> {
        let scope = match scope {
            Some(scope) => ScopeRef::Borrowed(scope),
            None => ScopeRef::Owned(Box::new(local_install_scope(false)?)),
        };
        // Construct the failure guard BEFORE the first cohort/pair work.
        let mut preflight = Self {
            scope: Some(scope),
            image: None,
            runtime: None,
        };
        preflight
            .scope
            .as_mut()
            .unwrap()
            .begin_installer_preflight(expected)
            .map_err(|_| refused())?;
        let runtime = capture_runtime_pair::current()?;
        let image = VerifiedKeeperImage::open(&runtime.guard_path, &runtime.guard_sha256)
            .map_err(|_| refused())?;
        preflight.image = Some(image);
        preflight.runtime = Some(runtime);
        Ok(preflight)
    }
    pub(crate) fn runtime(&self) -> &capture_runtime_pair::CaptureRuntimeIdentity {
        self.runtime
            .as_ref()
            .expect("constructed installer runtime")
    }
    pub(crate) fn seal(mut self) -> Result<InstallerPublication<'a>, AppError> {
        self.scope
            .as_mut()
            .unwrap()
            .seal_installer_preflight()
            .map_err(|_| refused())?;
        Ok(InstallerPublication {
            scope: self.scope.take().unwrap(),
            finished: false,
        })
    }
}
impl LeafRunner for InstallerPreflight<'_> {
    fn output(
        &mut self,
        command: Command,
        stdout_cap: usize,
        stderr_cap: usize,
    ) -> io::Result<CommandResult> {
        let plan = IoPlan::new(Vec::new(), stdout_cap, stderr_cap)?;
        let scope = self.scope.as_mut().unwrap();
        let result = scope.run_installer_verified(command, plan, self.image.as_ref().unwrap());
        result.map_err(|error| match scope.first_fault() {
            Some(reason) => io::Error::new(error.kind(), reason),
            None => error,
        })
    }
}
impl Drop for InstallerPreflight<'_> {
    fn drop(&mut self) {
        if let Some(scope) = self.scope.as_mut() {
            let end = scope.hard_end();
            scope.begin_cancel(ProtocolError("SCOPE_PREFLIGHT_DROPPED"), end);
        }
    }
}
impl InstallerPublication<'_> {
    pub(crate) fn check_live(&mut self) -> Result<(), AppError> {
        self.scope
            .check_installer_publication()
            .map_err(|_| refused())
    }
    pub(crate) fn finish(mut self, result: Result<(), AppError>) -> Result<(), AppError> {
        let result = if matches!(&self.scope, ScopeRef::Owned(_)) {
            let resources = self.scope.finish().clean;
            let terminal = self.scope.publish_terminal(resources && result.is_ok());
            match result {
                Err(error) => Err(error),
                Ok(()) if resources && terminal => Ok(()),
                _ => Err(refused()),
            }
        } else {
            result
        };
        self.finished = true;
        result
    }
}
impl Drop for InstallerPublication<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let end = self.scope.hard_end();
            self.scope
                .begin_cancel(ProtocolError("SCOPE_INSTALLER_DROPPED"), end);
        }
    }
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
            scope: ScopeRef::Owned(Box::new(Scope::local_entry(end).map_err(|_| refused())?)),
            image: Image::Owned(image),
            end,
            policy: probe_policy(false),
        })
    }
}
impl<'a> CommandOwner<'a> {
    pub(crate) fn with_scope(scope: &'a mut Scope) -> Result<Self, AppError> {
        let at = crate::process_ownership::scope::monotonic_now().map_err(|_| refused())?;
        let end = scope.hard_end().min(at.saturating_add(STANDALONE_NS));
        scope.tighten_end(end);
        let identity = capture_runtime_pair::current()?;
        let image = VerifiedKeeperImage::open(&identity.guard_path, &identity.guard_sha256)
            .map_err(|_| refused())?;
        Ok(Self {
            scope: ScopeRef::Borrowed(scope),
            image: Image::Owned(image),
            end,
            policy: probe_policy(false),
        })
    }
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
        let end = inherited
            .min(at.saturating_add(HOOK_NS))
            .min(scope.hard_end());
        scope.tighten_end(end);
        scope
            .set_hook_setup_directory(crate::capture_invocations::hook_setup_directory()?)
            .map_err(|_| refused())?;
        Ok(Self {
            scope: ScopeRef::Borrowed(scope),
            image: Image::Borrowed(runtime.keeper()),
            end,
            policy: probe_policy(true),
        })
    }
    pub(crate) fn finish_clean(&mut self) -> bool {
        self.scope.finish().clean
    }
    #[cfg(feature = "dev-tools")]
    pub(crate) fn failure_probe(
        &self,
        phase: crate::capture::HookFailurePhase,
    ) -> crate::capture::HookFailureProbe {
        crate::capture::HookFailureProbe {
            phase,
            owner_fault: self
                .scope
                .first_fault()
                .map(|fault| crate::capture::HookOwnerFault::from_static(fault.0)),
            setup: self.scope.first_fault_setup_trace().map(hook_setup_probe),
        }
    }
    pub(crate) fn finish<T>(&mut self, result: Result<T, AppError>) -> Result<T, AppError> {
        finish_result(result, self.finish_clean())
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
#[cfg(feature = "dev-tools")]
fn hook_setup_probe(
    trace: crate::process_ownership::group::SetupTrace,
) -> crate::capture::HookSetupProbe {
    let offset = |at: u64| at.checked_sub(trace.setup_start_ns);
    crate::capture::HookSetupProbe {
        fault_state: trace.fault_origin.map(|(_, state)| match state {
            "AWAIT_READY" => crate::capture::HookGroupState::AwaitReady,
            "READY" => crate::capture::HookGroupState::Ready,
            "JOINED" => crate::capture::HookGroupState::Joined,
            "PARKED" => crate::capture::HookGroupState::Parked,
            "CLEANING" => crate::capture::HookGroupState::Cleaning,
            "GONE" => crate::capture::HookGroupState::Gone,
            "RELEASED" => crate::capture::HookGroupState::Released,
            "FAILED" => crate::capture::HookGroupState::Failed,
            _ => crate::capture::HookGroupState::Other,
        }),
        ready_window_ns: offset(trace.ready_end_ns),
        pre_spawn_offset_ns: offset(trace.pre_spawn_ns),
        spawn_return_offset_ns: trace.spawn_return_ns.and_then(offset),
        first_ready_poll_offset_ns: trace.first_ready_poll_ns.and_then(offset),
        deadline_offset_ns: trace.deadline_observed.and_then(|(_, at)| offset(at)),
        ready_frame_observed: trace.ready_frame_observed,
        ready_eof_observed: trace.ready_eof_observed,
        deadline_edge: trace.deadline_observed.and_then(|(edge, _)| match edge {
            "READY_BEFORE_RECEIVE" => Some(crate::capture::HookReadyEdge::BeforeReceive),
            "READY_AFTER_RECEIVE" => Some(crate::capture::HookReadyEdge::AfterReceive),
            "PARK_BEFORE_RECEIVE" => Some(crate::capture::HookReadyEdge::ParkBeforeReceive),
            "PARK_AFTER_RECEIVE" => Some(crate::capture::HookReadyEdge::ParkAfterReceive),
            _ => None,
        }),
        park_end_offset_ns: trace.park_end_ns.and_then(offset),
        workload_spawn_return_offset_ns: trace.workload_spawn_return_ns.and_then(offset),
        first_park_poll_offset_ns: trace.first_park_poll_ns.and_then(offset),
        park_frame_observed: trace.park_end_ns.map(|_| trace.park_frame_observed),
    }
}
impl LeafRunner for CommandOwner<'_> {
    fn repository_outputs(
        &mut self,
        commands: [Command; 3],
    ) -> Option<io::Result<[CommandResult; 3]>> {
        if self.policy != CleanupPolicy::HookShort {
            return None;
        }
        let image = match &self.image {
            Image::Owned(image) => image,
            Image::Borrowed(image) => image,
        };
        let result = self.scope.run_verified_repository(commands, image);
        Some(result.map_err(|error| match self.scope.first_fault() {
            Some(reason) => io::Error::new(error.kind(), reason),
            None => error,
        }))
    }
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
        let plan = IoPlan::new(Vec::new(), stdout_cap, stderr_cap)?;
        let result = if self.policy == CleanupPolicy::HookShort {
            self.scope.run_verified(command, plan, image)
        } else {
            self.scope
                .run_verified_with_policy(command, plan, image, self.policy)
        };
        // Preserve only the owner's static fault type. Generic error bodies
        // stay unexamined and cannot become public probe diagnostics.
        result.map_err(|error| match self.scope.first_fault() {
            Some(reason) => io::Error::new(error.kind(), reason),
            None => error,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(feature = "dev-tools")]
    fn hook_probe_preserves_setup_offsets_without_restarting_the_ready_window() {
        let probe = hook_setup_probe(crate::process_ownership::group::SetupTrace {
            setup_start_ns: 100,
            ready_end_ns: 120,
            pre_spawn_ns: 115,
            spawn_return_ns: Some(125),
            first_ready_poll_ns: Some(127),
            last_ready_before_ns: Some(127),
            last_ready_after_ns: None,
            ready_frame_observed: false,
            ready_eof_observed: false,
            deadline_observed: Some(("READY_BEFORE_RECEIVE", 127)),
            fault_origin: None,
            ..Default::default()
        });
        assert_eq!(probe.ready_window_ns, Some(20));
        assert_eq!(probe.pre_spawn_offset_ns, Some(15));
        assert_eq!(probe.spawn_return_offset_ns, Some(25));
        assert_eq!(probe.first_ready_poll_offset_ns, Some(27));
        assert_eq!(probe.deadline_offset_ns, Some(27));
        assert!(matches!(
            probe.deadline_edge,
            Some(crate::capture::HookReadyEdge::BeforeReceive)
        ));
        assert!(!probe.ready_frame_observed);
    }

    #[test]
    #[cfg(feature = "dev-tools")]
    fn hook_probe_does_not_invent_offsets_for_missing_or_backwards_observations() {
        let probe = hook_setup_probe(crate::process_ownership::group::SetupTrace {
            setup_start_ns: 100,
            ready_end_ns: 99,
            pre_spawn_ns: 0,
            spawn_return_ns: None,
            first_ready_poll_ns: Some(98),
            last_ready_before_ns: None,
            last_ready_after_ns: None,
            ready_frame_observed: false,
            ready_eof_observed: false,
            deadline_observed: None,
            fault_origin: None,
            ..Default::default()
        });
        assert!(probe.ready_window_ns.is_none());
        assert!(probe.pre_spawn_offset_ns.is_none());
        assert!(probe.spawn_return_offset_ns.is_none());
        assert!(probe.first_ready_poll_offset_ns.is_none());
        assert!(probe.deadline_offset_ns.is_none());
        assert!(probe.deadline_edge.is_none());
    }
    #[test]
    #[cfg(feature = "dev-tools")]
    fn failure_setup_trace_state_is_closed_and_missing_origin_stays_unknown() {
        use crate::capture::HookGroupState;
        use crate::process_ownership::group::SetupTrace;
        assert!(hook_setup_probe(SetupTrace::default())
            .fault_state
            .is_none());
        for (state, expected) in [
            ("AWAIT_READY", HookGroupState::AwaitReady),
            ("READY", HookGroupState::Ready),
            ("JOINED", HookGroupState::Joined),
            ("PARKED", HookGroupState::Parked),
            ("CLEANING", HookGroupState::Cleaning),
            ("GONE", HookGroupState::Gone),
            ("RELEASED", HookGroupState::Released),
            ("FAILED", HookGroupState::Failed),
            ("PRIVATE_RAW_CANARY", HookGroupState::Other),
        ] {
            let probe = hook_setup_probe(SetupTrace {
                fault_origin: Some(("GROUP_DEADLINE", state)),
                ..Default::default()
            });
            assert_eq!(probe.fault_state, Some(expected));
            assert!(!serde_json::to_string(&probe).unwrap().contains("PRIVATE"));
        }
    }
    #[test]
    #[cfg(feature = "dev-tools")]
    fn hook_probe_retains_late_park_observations_without_rebasing_the_end() {
        use crate::capture::{HookGroupState, HookReadyEdge};
        use crate::process_ownership::group::SetupTrace;
        let probe = hook_setup_probe(SetupTrace {
            setup_start_ns: 100,
            park_end_ns: Some(150),
            workload_spawn_return_ns: Some(155),
            first_park_poll_ns: Some(158),
            deadline_observed: Some(("PARK_BEFORE_RECEIVE", 158)),
            fault_origin: Some(("GROUP_DEADLINE", "JOINED")),
            ..Default::default()
        });
        assert_eq!(probe.fault_state, Some(HookGroupState::Joined));
        assert_eq!(probe.park_end_offset_ns, Some(50));
        assert_eq!(probe.workload_spawn_return_offset_ns, Some(55));
        assert_eq!(probe.first_park_poll_offset_ns, Some(58));
        assert_eq!(probe.deadline_offset_ns, Some(58));
        assert_eq!(probe.park_frame_observed, Some(false));
        assert!(matches!(
            probe.deadline_edge,
            Some(HookReadyEdge::ParkBeforeReceive)
        ));
        let observed = hook_setup_probe(SetupTrace {
            setup_start_ns: 100,
            park_end_ns: Some(150),
            park_frame_observed: true,
            deadline_observed: Some(("PARK_AFTER_RECEIVE", 151)),
            ..Default::default()
        });
        assert_eq!(observed.park_frame_observed, Some(true));
        assert!(matches!(
            observed.deadline_edge,
            Some(HookReadyEdge::ParkAfterReceive)
        ));
        let missing = hook_setup_probe(SetupTrace::default());
        assert!(missing.park_end_offset_ns.is_none());
        assert!(missing.workload_spawn_return_offset_ns.is_none());
        assert!(missing.first_park_poll_offset_ns.is_none());
        assert!(missing.park_frame_observed.is_none());
        let backwards = hook_setup_probe(SetupTrace {
            setup_start_ns: 100,
            park_end_ns: Some(99),
            workload_spawn_return_ns: Some(98),
            first_park_poll_ns: Some(97),
            ..Default::default()
        });
        assert!(backwards.park_end_offset_ns.is_none());
        assert!(backwards.workload_spawn_return_offset_ns.is_none());
        assert!(backwards.first_park_poll_offset_ns.is_none());
    }
    #[test]
    fn installer_dropped_unsealed_handle_keeps_its_borrowed_owner_failed() {
        let end = crate::process_ownership::scope::monotonic_now().unwrap() + 4_000_000_000;
        let mut scope = Scope::with_owner(Default::default(), end);
        scope.begin_installer_preflight(1).unwrap();
        {
            // Actual partial-construction guard, before any image is prepared.
            let _preflight = InstallerPreflight {
                scope: Some(ScopeRef::Borrowed(&mut scope)),
                image: None,
                runtime: None,
            };
        }
        assert_eq!(
            scope.first_fault(),
            Some(ProtocolError("SCOPE_PREFLIGHT_DROPPED"))
        );
        assert!(scope.report().admission_closed);
        assert!(!scope.report().clean);
        assert!(!scope.publish_terminal(true));
    }
    #[test]
    fn version_probe_failure_keeps_first_reason_when_finish_is_unproved() {
        let original: Result<(), AppError> = Err(version_probe_error(io::Error::other(
            ProtocolError("SCOPE_ADMISSION"),
        )));
        let error = finish_result(original, false).unwrap_err();
        let value: serde_json::Value = serde_json::from_str(&error.to_json()).unwrap();
        assert_eq!(value["details"]["reason"], "admission_refused");
        assert_eq!(value["details"]["cleanup_status"], "unverified");
        assert_eq!(value["details"]["cleanup_unverified"], true);
        assert!(!error.to_json().contains("auth login"));
        assert!(finish_result(Ok(()), false).is_err());
    }
    #[test]
    fn process_ownership_command_probe_selection_keeps_hooks_short() {
        assert_eq!(probe_policy(false), CleanupPolicy::CommandProbe);
        assert_eq!(probe_policy(true), CleanupPolicy::HookShort);
        assert_eq!(STANDALONE_NS, 1_000_000_000);
        assert_eq!(HOOK_NS, 500_000_000);
    }
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
