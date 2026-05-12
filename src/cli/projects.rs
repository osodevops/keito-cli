use clap::{Args, Subcommand};

#[derive(Args)]
#[command(after_long_help = "\
NOTE: Tasks in Keito are workspace-global, not scoped to a project. \
Use `keito projects tasks` to list all available tasks.")]
pub struct ProjectsCommand {
    #[command(subcommand)]
    pub command: ProjectsSubcommand,
}

#[derive(Subcommand)]
pub enum ProjectsSubcommand {
    /// List available projects
    #[command(long_about = "\
List available projects in the current workspace.

AGENT DISCOVERY WORKFLOW:
  $ keito projects list --json | jq '.[].id'
  # use a project ID with `keito time start --project <ID>`

EXAMPLE:
  $ keito projects list --json
  [
    {\"id\": \"prj_abc\", \"name\": \"Acme Website\", \"code\": \"ACME\", ...},
    ...
  ]")]
    List {
        /// Max results to return
        #[arg(long)]
        limit: Option<u32>,

        /// Filter by client ID
        #[arg(long)]
        client: Option<String>,
    },

    /// Create a project
    #[command(long_about = "\
Create a project for a client.

Requires a Keito API key with manager permissions. The API assigns default \
workspace tasks unless --task is supplied one or more times.

EXAMPLE:
  keito projects create \"Acme Website\" --client cli_abc --code ACME --json")]
    Create {
        /// Project name
        name: String,

        /// Client ID
        #[arg(long)]
        client: String,

        /// Project code
        #[arg(long)]
        code: Option<String>,

        /// Project notes
        #[arg(long)]
        notes: Option<String>,

        /// Override billable status
        #[arg(long)]
        billable: Option<bool>,

        /// Assign a task ID. Can be used multiple times.
        #[arg(long = "task")]
        tasks: Vec<String>,
    },

    /// Show project details
    #[command(long_about = "\
Show details for a single project.

The project argument accepts a name, code, or numeric ID. Resolution \
is case-insensitive: \"acme\", \"ACME\", and \"prj_abc\" all work.

EXAMPLE:
  keito projects show acme --json
  keito projects show ACME
  keito projects show prj_abc")]
    Show {
        /// Project name, code, or ID
        #[arg(long_help = "\
Project name, code, or numeric ID. Resolution is case-insensitive.")]
        project: String,
    },

    /// List tasks (global — not filtered by project)
    #[command(long_about = "\
List tasks available in the current workspace.

IMPORTANT: Tasks are workspace-global in Keito, not scoped to any \
particular project. Every task returned here can be used with any project.

EXAMPLE:
  $ keito projects tasks --json
  [
    {\"id\": \"tsk_001\", \"name\": \"Development\", ...},
    {\"id\": \"tsk_002\", \"name\": \"Design\", ...},
    ...
  ]")]
    Tasks {
        /// Max results to return
        #[arg(long)]
        limit: Option<u32>,
    },
}
