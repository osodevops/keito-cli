//! Test fixture only. Uses the same compiled-anchor verifier as production.
use keito_cli::capture_runtime_pair::{self, CaptureRuntimeIdentity};
pub fn process_path() -> std::ffi::OsString {
    std::env::var_os("KEITO_TEST_PROCESS_PATH").expect("fixture Git PATH was not prepared")
}
pub fn pair() -> CaptureRuntimeIdentity {
    let core = std::env::var_os("KEITO_TEST_PAIR_CORE").expect(
        "Run Unix tests with tests/run-macos-isolated.sh or tests/support/run_paired_tests.py",
    );
    capture_runtime_pair::verify(
        std::path::Path::new(&core),
        // Plain cargo check/clippy must compile; the verifier rejects a missing
        // anchor at runtime just as it rejects a wrong digest.
        option_env!("KEITO_RELEASE_GUARD_SHA256").unwrap_or(""),
        env!("CARGO_PKG_VERSION"),
        env!("KEITO_BUILD_TARGET"),
    )
    .expect("invalid source-fixture runtime pair")
}
