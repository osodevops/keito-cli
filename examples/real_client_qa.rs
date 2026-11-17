//! Offline native discovery only. Never starts a model session or signs in.
fn main() -> std::process::ExitCode {
    #[cfg(unix)]
    let passed = keito_cli::real_client_qa::run_offline();
    #[cfg(not(unix))]
    let passed = false;
    std::process::ExitCode::from(if passed { 0 } else { 1 })
}
