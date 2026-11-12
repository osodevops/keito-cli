use clap::Parser;
use cli::Cli;
use error::AppError;
use keito_cli::{capture_runtime_pair, cli, commands, error, output};
use std::io::IsTerminal;

fn main() {
    #[cfg(unix)]
    let mut entry_scope = None;
    let mut parser_exit = None;
    let mut result = capture_runtime_pair::bootstrap_image().and_then(|()| {
        #[cfg(unix)]
        {
            entry_scope = keito_cli::process_ownership::scope::Scope::entry_from_environment(1_000_000_000)
                .map_err(|_| AppError::Config("Command ownership attachment refused".into()))?;
        }
        let cli = match Cli::try_parse() {
            Ok(cli) => cli,
            Err(error) => { parser_exit = Some(error.exit_code()); let _ = error.print(); return Ok(()); }
        };
        if matches!(&cli.command,cli::Command::Capture(cli::capture::CaptureCommand {command:cli::capture::CaptureSubcommand::Hook{..}})) {
            // Hook persistence is deliberately synchronous and network-free.
            // Do not start the general command runtime's worker threads per
            // lifecycle boundary. A future async dependency must fail closed.
            use std::future::Future;
            let mut future=std::pin::pin!(run(cli, #[cfg(unix)] entry_scope.as_mut()));
            let mut context=std::task::Context::from_waker(std::task::Waker::noop());
            return match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(result)=>result,
                std::task::Poll::Pending=>Err(AppError::Config("Capture hook required unsupported asynchronous work; collection remains paused".into())),
            };
        }
        tokio::runtime::Runtime::new()
            .map_err(|_| AppError::Config("Cannot start command runtime".into()))?
            .block_on(run(cli, #[cfg(unix)] entry_scope.as_mut()))
    });
    #[cfg(unix)]
    if let Some(scope) = entry_scope.as_mut() {
        let resources = scope.finish().clean;
        if !scope.publish_terminal(resources && result.is_ok() && parser_exit.unwrap_or(0) == 0)
            && result.is_ok()
        {
            result = Err(AppError::Config(
                "Command ownership did not settle; collection remains paused".into(),
            ));
        }
    }
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
