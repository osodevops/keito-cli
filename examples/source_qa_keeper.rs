//! Developer QA sibling group keeper. Never installed or launched by default.
#[cfg(unix)]
fn main() {
    if keito_cli::source_qa_group::keeper_main(std::env::args_os().skip(1)).is_err() {
        // No argv/path or OS error may enter this helper's output.
        std::process::exit(1);
    }
}
#[cfg(not(unix))]
fn main() {
    std::process::exit(1);
}
