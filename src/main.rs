use clap::Parser;
use cli::Cli;
use error::AppError;
use keito_cli::{capture_runtime_pair, cli, commands, error, output};
use std::io::IsTerminal;

fn main() {
    let result = capture_runtime_pair::bootstrap_image().and_then(|()| {
        let cli = Cli::parse();
        if matches!(&cli.command,cli::Command::Capture(cli::capture::CaptureCommand {command:cli::capture::CaptureSubcommand::Hook{..}})) {
            // Hook persistence is deliberately synchronous and network-free.
            // Do not start the general command runtime's worker threads per
            // lifecycle boundary. A future async dependency must fail closed.
            use std::future::Future;
            let mut future=std::pin::pin!(run(cli));
            let mut context=std::task::Context::from_waker(std::task::Waker::noop());
            return match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(result)=>result,
                std::task::Poll::Pending=>Err(AppError::Config("Capture hook required unsupported asynchronous work; collection remains paused".into())),
            };
        }
        tokio::runtime::Runtime::new()
            .map_err(|_| AppError::Config("Cannot start command runtime".into()))?
            .block_on(run(cli))
    });
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

async fn run(cli: Cli) -> Result<(), AppError> {
    let output_mode = if cli.json || !std::io::stdout().is_terminal() {
        output::OutputMode::Json
    } else {
        output::OutputMode::Table
    };

    match cli.command {
        cli::Command::Capture(cmd) => commands::capture::run(cmd, &cli.global, output_mode).await,
        cli::Command::Auth(cmd) => commands::auth::run(cmd, &cli.global, output_mode).await,
        cli::Command::Clients(cmd) => commands::clients::run(cmd, &cli.global, output_mode).await,
        cli::Command::Time(cmd) => commands::time::run(cmd, &cli.global, output_mode).await,
        cli::Command::Projects(cmd) => commands::projects::run(cmd, &cli.global, output_mode).await,
        cli::Command::Skill(cmd) => commands::skill::run(cmd, &cli.global, output_mode).await,
    }
}
