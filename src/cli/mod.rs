pub mod auth;
pub mod clients;
pub mod projects;
pub mod time;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "keito",
    about = "Track billable time against the Keito platform — https://keito.ai",
    long_about = "\
Track billable time against the Keito platform.

keito is an agent-native CLI for the Keito v2 API. It is designed to be \
driven by AI agents as well as humans. Every command supports --json output \
and returns structured exit codes for programmatic error handling.",
    version,
    propagate_version = true,
    after_long_help = "\
ENVIRONMENT VARIABLES:
  KEITO_API_KEY          API key (takes precedence over config)
  KEITO_ACCOUNT_ID       Company/account ID for Keito-Account-Id
  KEITO_WORKSPACE_ID     Legacy alias for KEITO_ACCOUNT_ID

CONFIG FILE:
  platform config dir (for example ~/Library/Application Support/keito/config.toml on macOS)

EXIT CODES:
  0   Success
  1   Authentication error (missing or invalid API key)
  2   Invalid input (bad arguments, malformed duration)
  3   Conflict (e.g. timer already running on start)
  4   Not found (project, task, or entry does not exist)
  5   Rate limited (retry after a moment)
  6   Server error (Keito API 5xx)
  7   Network error (connection failed, timeout)
  8   Configuration error (missing config, bad TOML)

QUICK START (AGENT):
  keito auth status --json          # verify credentials
  keito projects list --json        # discover project IDs
  keito projects tasks --json       # discover task IDs
  keito time start --project <ID> --task <ID> --json
  keito time running --json         # check active timer
  keito time stop --json            # stop when done

QUICK START (HUMAN):
  keito auth login                  # one-time interactive setup
  keito projects list               # browse projects
  keito time start --project myproj --task dev
  keito time stop"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Output as JSON (default when stdout is piped)
    #[arg(
        long,
        global = true,
        long_help = "\
Output as JSON. When stdout is piped to another process, JSON output is \
enabled automatically. Use this flag to force JSON output in a terminal."
    )]
    pub json: bool,

    #[command(flatten)]
    pub global: GlobalFlags,
}

#[derive(Parser, Clone)]
pub struct GlobalFlags {
    /// Override account/company ID
    #[arg(
        long,
        global = true,
        long_help = "\
Override the account/company ID for this invocation. The value is sent as \
Keito-Account-Id. Resolution order: this flag > KEITO_ACCOUNT_ID env var > \
KEITO_WORKSPACE_ID env var > config account_id > config workspace_id. Find \
this value in Keito under Settings > API & Developers > Company ID."
    )]
    pub workspace: Option<String>,

    /// Suppress non-essential output
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Enable debug logging
    #[arg(long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Manage authentication (login, logout, status, whoami)
    Auth(auth::AuthCommand),
    /// Browse clients
    Clients(clients::ClientsCommand),
    /// Track time entries (start, stop, log, list, running)
    Time(time::TimeCommand),
    /// Browse projects and tasks
    Projects(projects::ProjectsCommand),
}
