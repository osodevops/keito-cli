//! Closed CLI entry classification; no child API is exposed before parsing.
use crate::{
    cli,
    process_ownership::{
        protocol::ProtocolError,
        scope::{self, Scope},
    },
};
use std::io;

pub struct PendingEntry {
    origin: u64,
    ceiling: u64,
    scope: Option<Scope>,
}
fn direct_installer(command: Option<&cli::Command>) -> bool {
    matches!(
        command,
        Some(cli::Command::Skill(cli::skill::SkillCommand {
            command: cli::skill::SkillSubcommand::Install { .. },
        }))
    )
}
fn selected_end(origin: u64, ceiling: u64, installer: bool) -> io::Result<u64> {
    origin
        .checked_add(if installer {
            scope::INSTALL_OPERATION_NS
        } else {
            1_000_000_000
        })
        .map(|end| end.min(ceiling))
        .ok_or_else(|| io::Error::other("SCOPE_CLOCK"))
}
impl PendingEntry {
    pub fn attach() -> io::Result<Self> {
        let origin = scope::monotonic_now()?;
        let ceiling = selected_end(origin, u64::MAX, true)?;
        let scope = Scope::attach_at(origin, ceiling)?;
        Ok(Self {
            origin,
            ceiling,
            scope,
        })
    }
    /// Consumed once. A failed selection still returns its retained attachment.
    pub fn select(mut self, command: Option<&cli::Command>) -> (Option<Scope>, io::Result<()>) {
        let installer = direct_installer(command);
        let result = (|| {
            let end = selected_end(self.origin, self.ceiling, installer)?;
            if self.scope.is_none() && installer {
                self.scope = Some(Scope::local_entry(end)?);
            }
            if let Some(scope) = self.scope.as_mut() {
                scope.refine_entry(end)?;
            }
            Ok(())
        })();
        if result.is_err() {
            if let Some(scope) = self.scope.as_mut() {
                if !scope.failed() {
                    scope.begin_cancel(ProtocolError("SCOPE_ENTRY_SELECTION"), 0);
                }
            }
        }
        (self.scope.take(), result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn installer_entry_classifier_is_typed_and_preserves_the_original_origin() {
        let install =
            cli::Cli::try_parse_from(["keito", "skill", "install", "--agent", "codex"]).unwrap();
        let auth = cli::Cli::try_parse_from(["keito", "auth", "status"]).unwrap();
        assert!(direct_installer(Some(&install.command)));
        assert!(!direct_installer(Some(&auth.command)));
        assert!(!direct_installer(None));
        assert_eq!(
            selected_end(100_000_000, u64::MAX, true).unwrap(),
            30_100_000_000
        );
        assert_eq!(
            selected_end(100_000_000, u64::MAX, false).unwrap(),
            1_100_000_000
        );
        assert_eq!(
            selected_end(100_000_000, 300_000_000, true).unwrap(),
            300_000_000
        );
        assert!(selected_end(u64::MAX, u64::MAX, true).is_err());
    }
    #[test]
    fn installer_policy_preserves_earlier_parents_and_other_entry_limits() {
        let origin = 100_000_000;
        assert_eq!(
            selected_end(origin, u64::MAX, true).unwrap(),
            30_100_000_000
        );
        assert_eq!(
            selected_end(origin, 4_000_000_000, true).unwrap(),
            4_000_000_000
        );
        assert_eq!(
            selected_end(origin, u64::MAX, false).unwrap(),
            1_100_000_000
        );
        assert_eq!(
            selected_end(origin, 300_000_000, false).unwrap(),
            300_000_000
        );
        assert!(selected_end(u64::MAX - 20_000_000_000, u64::MAX, true).is_err());
    }
}
