use clap::{Args, Subcommand};

#[derive(Args)]
#[command(after_long_help = "\
CREDENTIAL RESOLUTION ORDER:
  1. KEITO_API_KEY environment variable (highest priority)
  2. config.toml api_key field

Agents should prefer setting KEITO_API_KEY in the environment rather \
than running `keito auth login`, which requires interactive input.")]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: AuthSubcommand,
}

#[derive(Subcommand)]
pub enum AuthSubcommand {
    /// Store API key and configure account ID (interactive, one-time setup)
    #[command(long_about = "\
Store API key and configure account ID (interactive, one-time setup).

This command prompts for an API key and account/company ID, validates them \
against the production v2 API, and stores them in the platform config file. \
For non-interactive / agent use, set the KEITO_API_KEY and KEITO_ACCOUNT_ID \
environment variables instead.

Find the account/company ID in Keito under Settings > API & Developers > \
Company ID.

EXAMPLE:
  keito auth login              # interactive prompt
  export KEITO_API_KEY=kto_xxx  # agent alternative (no login needed)
  export KEITO_ACCOUNT_ID=co_xxx")]
    Login,

    /// Remove stored credentials from config
    Logout,

    /// Check authentication status and credential source
    #[command(long_about = "\
Check authentication status and credential source.

Returns which credential source is active and whether it is valid. \
Useful as a health check before starting a session.

EXAMPLE (JSON):
  $ keito auth status --json
  {
    \"authenticated\": true,
    \"api_key_source\": \"environment variable\",
    \"account_id\": \"co_abc123\",
    \"workspace_id\": \"co_abc123\"
  }

EXIT CODES:
  0   Authenticated
  1   Not authenticated (no valid credentials found)")]
    Status,

    /// Show current user identity and account info
    #[command(long_about = "\
Show current user identity and account info.

Calls the Keito API to return the user profile associated with the \
current credentials.

EXAMPLE (JSON):
  $ keito auth whoami --json
  {
    \"id\": \"usr_abc123\",
    \"first_name\": \"Jane\",
    \"last_name\": \"Doe\",
    \"email\": \"jane@example.com\",
    \"company\": { \"id\": \"co_abc123\", \"name\": \"Acme Corp\" }
  }")]
    Whoami,
}
