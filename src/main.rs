use clap::Parser;
use cli::Cli;
use error::AppError;
use keito_cli::{capture_runtime_pair, cli, commands, error, output};
use std::io::IsTerminal;

fn local_command(cli: &Cli) -> bool {
    match &cli.command {
        cli::Command::Capture(command) => matches!(
            &command.command,
            cli::capture::CaptureSubcommand::Hook { .. }
                | cli::capture::CaptureSubcommand::Status
                | cli::capture::CaptureSubcommand::Usage { .. }
                | cli::capture::CaptureSubcommand::Pause { .. }
                | cli::capture::CaptureSubcommand::Resume { .. }
        ),
        cli::Command::Skill(command) => match &command.command {
            cli::skill::SkillSubcommand::Runtime => true,
            cli::skill::SkillSubcommand::Install { source, agent, .. } => {
                cli.global.quiet && !agent.is_empty() && matches!(source.trim(), "" | "bundled")
            }
            _ => false,
        },
        _ => false,
    }
}

fn poll_local(
    future: impl std::future::Future<Output = Result<(), AppError>>,
    hook: bool,
) -> Result<(), AppError> {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => Err(AppError::Config(
            if hook {
                "Capture hook required unsupported asynchronous work; collection remains paused"
            } else {
                "Local command required unsupported asynchronous work; operation refused"
            }
            .into(),
        )),
    }
}

fn main() {
    #[cfg(unix)]
    let mut entry_scope = None;
    let mut parser_exit = None;
    let result = capture_runtime_pair::bootstrap_image().and_then(|()| {
        #[cfg(unix)]
        let pending = keito_cli::process_entry::PendingEntry::attach()
            .map_err(|_| AppError::Config("Command ownership attachment refused".into()))?;
        let parsed = Cli::try_parse();
        if let Err(error) = &parsed {
            parser_exit = Some(error.exit_code());
        }
        #[cfg(unix)]
        {
            let (scope, selection) = pending.select(parsed.as_ref().ok().map(|cli| &cli.command));
            entry_scope = scope;
            selection.map_err(|_| AppError::Config("Command ownership policy refused".into()))?;
        }
        let cli = match parsed {
            Ok(cli) => cli,
            Err(error) => {
                parser_exit = Some(error.exit_code());
                let _ = error.print();
                return Ok(());
            }
        };
        if local_command(&cli) {
            // These reviewed handlers perform only synchronous local work.
            // Avoid the general worker pool and refuse an unexpected Pending.
            // Network and unclassified commands retain the normal runtime.
            let hook = matches!(
                &cli.command,
                cli::Command::Capture(cli::capture::CaptureCommand {
                    command: cli::capture::CaptureSubcommand::Hook { .. },
                })
            );
            return poll_local(
                run(
                    cli,
                    #[cfg(unix)]
                    entry_scope.as_mut(),
                ),
                hook,
            );
        }
        tokio::runtime::Runtime::new()
            .map_err(|_| AppError::Config("Cannot start command runtime".into()))?
            .block_on(run(
                cli,
                #[cfg(unix)]
                entry_scope.as_mut(),
            ))
    });
    #[cfg(unix)]
    let result = if let Some(scope) = entry_scope.as_mut() {
        let resources = scope.finish().clean;
        if !scope.publish_terminal(resources && result.is_ok() && parser_exit.unwrap_or(0) == 0)
            && result.is_ok()
        {
            Err(AppError::Config(
                "Command ownership did not settle; collection remains paused".into(),
            ))
        } else {
            result
        }
    } else {
        result
    };
    if let Some(code) = parser_exit {
        std::process::exit(code);
    }
    if let Err(e) = result {
        let json_mode = std::env::args().any(|a| a == "--json") || !std::io::stdout().is_terminal();
        if json_mode {
            eprintln!("{}", e.to_json());
        } else {
            use colored::Colorize;
            eprintln!("{} {}", "error:".red().bold(), e);
        }
        std::process::exit(e.exit_code());
    }
}

async fn run(
    cli: Cli,
    #[cfg(unix)] scope: Option<&mut keito_cli::process_ownership::scope::Scope>,
) -> Result<(), AppError> {
    let output_mode = if cli.json || !std::io::stdout().is_terminal() {
        output::OutputMode::Json
    } else {
        output::OutputMode::Table
    };

    match cli.command {
        cli::Command::Capture(cmd) => {
            #[cfg(unix)]
            {
                commands::capture::run_with_scope(cmd, &cli.global, output_mode, scope).await
            }
            #[cfg(not(unix))]
            {
                commands::capture::run(cmd, &cli.global, output_mode).await
            }
        }
        cli::Command::Auth(cmd) => commands::auth::run(cmd, &cli.global, output_mode).await,
        cli::Command::Clients(cmd) => commands::clients::run(cmd, &cli.global, output_mode).await,
        cli::Command::Time(cmd) => commands::time::run(cmd, &cli.global, output_mode).await,
        cli::Command::Projects(cmd) => commands::projects::run(cmd, &cli.global, output_mode).await,
        cli::Command::Skill(cmd) => {
            #[cfg(unix)]
            {
                commands::skill::run_with_scope(cmd, &cli.global, output_mode, scope).await
            }
            #[cfg(not(unix))]
            {
                commands::skill::run(cmd, &cli.global, output_mode).await
            }
        }
    }
}

#[cfg(test)]
mod local_startup_tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("keito").chain(arguments.iter().copied())).unwrap()
    }

    #[test]
    fn local_startup_known_local_commands_need_no_async_workers() {
        for arguments in [
            vec!["skill", "runtime"],
            vec![
                "--quiet", "skill", "install", "--source", "bundled", "--agent", "codex",
            ],
            vec!["capture", "status"],
            vec!["capture", "pause", "--repo"],
            vec!["capture", "resume", "--repo"],
            vec!["capture", "usage", "status", "--repo", "--agent", "codex"],
            vec![
                "capture",
                "usage",
                "enable",
                "--repo",
                "--agent",
                "codex",
                "--profile-home",
                "/synthetic/home",
                "--harness-executable",
                "/synthetic/codex",
                "--confirm",
            ],
        ] {
            assert!(local_command(&parsed(&arguments)), "{arguments:?}");
        }
    }

    #[test]
    fn local_startup_general_and_network_commands_keep_async_workers() {
        for arguments in [
            vec!["auth", "status"],
            vec!["capture", "sync"],
            vec!["capture", "worker"],
            vec![
                "capture",
                "setup",
                "--agent",
                "codex",
                "--project",
                "p",
                "--task",
                "t",
                "--confirm",
            ],
            vec!["skill", "doctor", "--live"],
            vec![
                "skill", "install", "--source", "bundled", "--agent", "codex",
            ],
            vec![
                "--quiet",
                "skill",
                "install",
                "--source",
                "external/repository",
                "--agent",
                "codex",
            ],
            vec!["--quiet", "skill", "install", "--source", "bundled"],
        ] {
            assert!(!local_command(&parsed(&arguments)), "{arguments:?}");
        }
    }

    #[test]
    fn local_startup_existing_hook_is_still_synchronous() {
        assert!(local_command(&parsed(&[
            "capture",
            "hook",
            "--agent",
            "codex",
            "--event",
            "prompt",
            "--harness-version",
            "0.167.0"
        ])));
    }

    #[test]
    fn local_startup_ready_future_does_not_enter_tokio() {
        poll_local(
            async {
                assert!(tokio::runtime::Handle::try_current().is_err());
                Ok(())
            },
            false,
        )
        .unwrap();
    }

    #[test]
    fn local_startup_pending_future_is_polled_once_and_refused() {
        let mut polls = 0;
        let future = std::future::poll_fn(|_| {
            polls += 1;
            std::task::Poll::Pending
        });
        assert!(matches!(
            poll_local(future, false),
            Err(AppError::Config(_))
        ));
        assert_eq!(polls, 1);
    }

    #[test]
    fn local_startup_ready_failure_is_preserved() {
        assert!(
            matches!(poll_local(async { Err(AppError::Conflict("finite-refusal".into())) }, false), Err(AppError::Conflict(value)) if value == "finite-refusal")
        );
    }
}
