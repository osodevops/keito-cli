//! Platform boundary for recoverable repository mapping publication.
/// Holds the private setup lock until successful commit or conflict-aware rollback.
pub struct MappingIds<'a> {
    pub workspace: &'a str,
    pub client: &'a str,
    pub project: &'a str,
    pub task: &'a str,
}

#[cfg(unix)]
#[path = "capture_mapping_unix.rs"]
mod implementation;
#[cfg(not(unix))]
mod implementation {
    use super::MappingIds;
    use crate::error::AppError;
    use std::path::Path;
    pub struct MappingPublication;
    fn unsupported() -> AppError {
        AppError::Config("Native coding-agent capture setup is not supported on this platform. Manual CLI time workflows remain available.".into())
    }
    impl MappingPublication {
        pub fn begin(_: &Path, _: &MappingIds<'_>) -> Result<Self, AppError> {
            Err(unsupported())
        }
        pub fn commit(&mut self) -> Result<(), AppError> {
            Err(unsupported())
        }
        pub fn rollback(&mut self) -> Result<(), AppError> {
            Err(unsupported())
        }
    }
}
pub use implementation::MappingPublication;
