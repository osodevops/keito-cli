//! Tool result interpretation after ownership, never a process-spawn fallback.
use super::scope::CommandResult;
use std::{io, process::Command};

pub(crate) trait LeafRunner {
    /// Optional fixed repository cohort. None preserves the original sequential
    /// resolver, including short-circuiting before later commands are executed.
    fn repository_outputs(
        &mut self,
        _commands: [Command; 3],
    ) -> Option<io::Result<[CommandResult; 3]>> {
        None
    }
    fn output(
        &mut self,
        command: Command,
        stdout_cap: usize,
        stderr_cap: usize,
    ) -> io::Result<CommandResult>;
}
/// Git config --get-regexp uses exit1 with empty output for no matches. Every
/// ownership/read/deadline error is propagated BEFORE reaching this classifier.
pub(crate) fn text(result: CommandResult, allow_empty_exit_one: bool) -> io::Result<String> {
    if !result.status.success()
        && !(allow_empty_exit_one
            && result.status.code() == Some(1)
            && result.output.stdout.is_empty()
            && result.output.stderr.is_empty())
    {
        return Err(io::Error::other("PROBE_EXIT"));
    }
    String::from_utf8(result.output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|_| io::Error::other("PROBE_UTF8"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_ownership::io::CommandOutput;
    use std::os::unix::process::ExitStatusExt;
    fn result(code: i32, out: &[u8], err: &[u8]) -> CommandResult {
        CommandResult {
            status: std::process::ExitStatus::from_raw(code << 8),
            output: CommandOutput {
                stdout: out.into(),
                stderr: err.into(),
            },
        }
    }
    #[test]
    fn r40_only_the_known_empty_git_exit_can_be_an_empty_identity_result() {
        assert_eq!(text(result(1, b"", b""), true).unwrap(), "");
        assert!(text(result(1, b"", b"error"), true).is_err());
        assert!(text(result(1, b"unexpected", b""), true).is_err());
        assert!(text(result(128, b"", b""), true).is_err());
        assert!(text(result(1, b"", b""), false).is_err());
        assert_eq!(text(result(0, b" root\n", b""), false).unwrap(), "root");
    }
    #[test]
    fn r40_invalid_text_and_signals_are_not_valid_empty_remotes() {
        assert!(text(result(0, &[255], b""), true).is_err());
        let mut r = result(0, b"", b"");
        r.status = std::process::ExitStatus::from_raw(15);
        assert!(text(r, true).is_err());
    }
}
