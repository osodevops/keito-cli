mod api;
mod cli;
mod commands;
mod config;
mod error;
mod output;
mod types;

use clap::Parser;
use cli::Cli;
use error::AppError;
use std::io::IsTerminal;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
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
        cli::Command::Auth(cmd) => commands::auth::run(cmd, &cli.global, output_mode).await,
        cli::Command::Clients(cmd) => commands::clients::run(cmd, &cli.global, output_mode).await,
        cli::Command::Time(cmd) => commands::time::run(cmd, &cli.global, output_mode).await,
        cli::Command::Projects(cmd) => commands::projects::run(cmd, &cli.global, output_mode).await,
        cli::Command::Skill(cmd) => commands::skill::run(cmd, &cli.global, output_mode).await,
    }
}
