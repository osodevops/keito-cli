//! Platform boundary for native adapter management; manual CLI stays portable.
use crate::capture_runtime_pair::CaptureRuntimeIdentity;
use crate::cli::skill::SkillAgent;
use serde::Serialize;
use std::path::Path;

pub struct InstallOptions<'a> {
    pub source_root: &'a Path,
    pub profile_home: &'a Path,
    pub agent: SkillAgent,
    pub runtime: &'a CaptureRuntimeIdentity,
    pub confirm_migration: bool,
    pub harness_version: Option<&'a str>,
}
#[derive(Debug, Serialize)]
pub struct InstallOutcome {
    pub generation: String,
    pub hooks_registered: usize,
    pub migrated: bool,
}
#[derive(Debug, Serialize)]
pub struct UninstallOutcome {
    pub hooks_removed: usize,
    pub payload_removed: bool,
    pub retained_generations: usize,
}
#[cfg(unix)]
#[path = "skill_adapter_unix.rs"]
mod implementation;
#[cfg(not(unix))]
mod implementation {
    use super::*;
    use crate::error::AppError;
    fn unsupported() -> AppError {
        AppError::Config("Native coding-agent capture installation is not supported on this platform. Manual CLI time workflows remain available.".into())
    }
    pub fn install(_: InstallOptions<'_>) -> Result<InstallOutcome, AppError> {
        Err(unsupported())
    }
    pub fn uninstall(_: &Path, _: SkillAgent) -> Result<UninstallOutcome, AppError> {
        Err(unsupported())
    }
    pub fn inspect(
        _: &Path,
        _: SkillAgent,
        _: Option<&CaptureRuntimeIdentity>,
    ) -> Result<bool, AppError> {
        Ok(false)
    }
}
#[cfg(all(unix, test))]
pub(crate) use implementation::tests;
pub use implementation::{inspect, install, uninstall};
