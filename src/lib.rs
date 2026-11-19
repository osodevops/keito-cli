mod adapter_files;
pub mod api;
pub mod capture;
mod capture_authority;
mod capture_busy;
#[cfg(unix)]
mod capture_command_owner;
mod capture_invocations;
pub mod capture_mapping;
pub mod capture_runtime_pair;
pub mod capture_supervisor;
mod capture_usage;
mod capture_usage_consent;
mod capture_usage_ledger;
#[cfg(unix)]
mod capture_usage_origin;
#[cfg(unix)]
mod capture_usage_profile;
#[cfg(all(test, unix))]
mod capture_usage_test_io;
pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod output;
#[cfg(unix)]
#[doc(hidden)]
pub mod process_entry;
#[cfg(unix)]
#[doc(hidden)]
pub mod process_ownership;
#[cfg(all(feature = "dev-tools", unix))]
pub mod real_client_qa;
pub mod skill_adapter;
#[cfg(all(feature = "dev-tools", unix))]
pub mod source_qa_group;
#[cfg(all(feature = "dev-tools", unix))]
pub mod source_qa_protocol;
#[cfg(all(feature = "dev-tools", unix))]
pub mod source_qa_scope;
pub mod types;

#[cfg(all(unix, feature = "capture-profile"))]
#[doc(hidden)]
pub mod capture_profile;
/// Internal numeric profiling; disabled shipping builds evaluate only the body.
#[doc(hidden)]
#[macro_export]
macro_rules! capture_profile_measure {
    (Ack, $body:expr, $accepted:expr) => {{
        #[cfg(all(unix, feature = "capture-profile"))]
        {
            $crate::capture_profile::measure_ack($accepted, || $body)
        }
        #[cfg(not(all(unix, feature = "capture-profile")))]
        {
            $body
        }
    }};
    ($phase:ident, $body:expr) => {{
        #[cfg(all(unix, feature = "capture-profile"))]
        {
            $crate::capture_profile::measure($crate::capture_profile::Phase::$phase, || $body)
        }
        #[cfg(not(all(unix, feature = "capture-profile")))]
        {
            $body
        }
    }};
}
