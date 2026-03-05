use clap::{Args, Subcommand};

#[derive(Args)]
#[command(after_long_help = "\
CREDENTIAL RESOLUTION ORDER:
  1. KEITO_API_KEY environment variable (highest priority)
  2. OS keyring (stored by `keito auth login`)
  3. ~/.config/keito/config.toml api_key field

Agents should prefer setting KEITO_API_KEY in the environment rather \
than running `keito auth login`, which requires interactive input.")]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: AuthSubcommand,
}

#[derive(Subcommand)]
pub enum AuthSubcommand {
    /// Store API key and configure workspace (interactive, one-time setup)
    #[command(long_about = "\
Store API key and configure workspace (interactive, one-time setup).

This command prompts for an API key and workspace ID, then stores them \
securely in the OS keyring. For non-interactive / agent use, set the \
KEITO_API_KEY and KEITO_WORKSPACE_ID environment variables instead.

EXAMPLE:
  keito auth login              # interactive prompt
  export KEITO_API_KEY=kto_xxx  # agent alternative (no login needed)")]
    Login,

    /// Remove stored credentials from keychain
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
    \"source\": \"environment\",
    \"workspace_id\": \"ws_abc123\"
  }

EXIT CODES:
  0   Authenticated
  1   Not authenticated (no valid credentials found)")]
    Status,

    /// Show current user identity and workspace info
    #[command(long_about = "\
Show current user identity and workspace info.

Calls the Keito API to return the user profile associated with the \
current credentials.

EXAMPLE (JSON):
  $ keito auth whoami --json
  {
    \"user_id\": \"usr_abc123\",
    \"name\": \"Jane Doe\",
    \"email\": \"jane@example.com\",
    \"workspace_id\": \"ws_abc123\",
    \"workspace_name\": \"Acme Corp\"
  }")]
    Whoami,
}
