use clap::{Args, Subcommand};

#[derive(Args)]
pub struct ClientsCommand {
    #[command(subcommand)]
    pub command: ClientsSubcommand,
}

#[derive(Subcommand)]
pub enum ClientsSubcommand {
    /// List active clients
    #[command(long_about = "\
List active clients available to the authenticated Keito account.

Use this before project discovery when an agent needs to map user-provided \
client context to Keito's project names and IDs.

API EFFECT:
  GET /api/v2/clients?is_active=true&per_page=200

EXAMPLE:
  keito clients list --json")]
    List {
        /// Max clients to display
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Create a client
    #[command(long_about = "\
Create a client in Keito.

Requires a Keito API key with manager permissions.

API EFFECT:
  POST /api/v2/clients

EXAMPLE:
  keito clients create \"Acme Ltd\" --currency GBP --json")]
    Create {
        /// Client name
        name: String,

        /// Client billing currency, such as USD or GBP
        #[arg(long)]
        currency: Option<String>,

        /// Client postal address
        #[arg(long)]
        address: Option<String>,
    },
}
