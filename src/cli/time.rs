use clap::{Args, Subcommand};

#[derive(Args)]
#[command(after_long_help = "\
AGENT WORKFLOW:
  1. keito time running --json      # check if a timer is already active
  2. keito time start ... --json    # start a new timer (exit 3 if one exists)
  3. ... perform work ...
  4. keito time stop --json         # stop the timer when done

Only one timer may be active at a time. Starting a timer while one is \
running returns exit code 3 (conflict).")]
pub struct TimeCommand {
    #[command(subcommand)]
    pub command: TimeSubcommand,
}

#[derive(Subcommand)]
pub enum TimeSubcommand {
    /// Start a timer for a project and task
    #[command(long_about = "\
Start a timer for a project and task.

Creates a running time entry. Only one timer may be active at a time; \
starting a second timer returns exit code 3 (conflict). The --project \
and --task flags accept a name, code, or numeric ID (case-insensitive).

EXAMPLE:
  $ keito time start --project \"Acme Website\" --task dev --json
  {
    \"id\": \"te_abc123\",
    \"project\": \"Acme Website\",
    \"task\": \"Development\",
    \"started_at\": \"2025-01-15T09:00:00Z\",
    \"is_running\": true
  }

EXIT CODES:
  0   Timer started
  3   Conflict — a timer is already running
  4   Project or task not found")]
    Start {
        /// Project name, code, or ID
        #[arg(
            long,
            long_help = "\
Project name, code, or numeric ID. Resolution is case-insensitive. \
Use `keito projects list --json` to discover available values."
        )]
        project: String,

        /// Task name or ID
        #[arg(
            long,
            long_help = "\
Task name or numeric ID. Resolution is case-insensitive. Tasks are \
global (not per-project). Use `keito projects tasks --json` to list."
        )]
        task: String,

        /// Description of work being performed
        #[arg(long)]
        notes: Option<String>,

        /// Override billable status
        #[arg(long)]
        billable: Option<bool>,
    },

    /// Stop the currently running timer
    #[command(long_about = "\
Stop the currently running timer.

Patches the active time entry to set is_running = false. Returns the \
completed entry with final duration.

EXAMPLE:
  $ keito time stop --json
  {
    \"id\": \"te_abc123\",
    \"project\": \"Acme Website\",
    \"task\": \"Development\",
    \"duration\": 1.5,
    \"is_running\": false
  }

EXIT CODES:
  0   Timer stopped (or discarded)
  4   No running timer found")]
    Stop {
        /// Append to or replace notes on the entry
        #[arg(long)]
        notes: Option<String>,

        /// Discard the running timer instead of saving it
        #[arg(
            long,
            long_help = "\
Discard the running timer instead of saving it. The time entry \
is deleted from Keito and no duration is recorded. Use this to \
abandon a timer that was started by mistake."
        )]
        discard: bool,
    },

    /// Log a completed time entry (duration-based, no timer)
    #[command(long_about = "\
Log a completed time entry with an explicit duration (no timer).

Use this to record time after the fact rather than using start/stop.

DURATION FORMATS:
  1.5    → 1 hour 30 minutes (decimal hours)
  1:30   → 1 hour 30 minutes (HH:MM)
  0:15   → 15 minutes
  0.25   → 15 minutes

EXAMPLE:
  $ keito time log --project acme --task dev --duration 1:30 \\
      --date 2025-01-15 --notes \"Fixed auth bug\" --json
  {
    \"id\": \"te_def456\",
    \"project\": \"Acme Website\",
    \"task\": \"Development\",
    \"duration\": 1.5,
    \"date\": \"2025-01-15\",
    \"is_running\": false
  }")]
    Log {
        /// Project name, code, or ID
        #[arg(long)]
        project: String,

        /// Task name or ID
        #[arg(long)]
        task: String,

        /// Duration in decimal hours (1.5) or HH:MM (1:30)
        #[arg(
            long,
            long_help = "\
Duration of the time entry. Accepts two formats:
  Decimal hours: 1.5 (= 1h 30m), 0.25 (= 15m)
  HH:MM format:  1:30 (= 1h 30m), 0:15 (= 15m)"
        )]
        duration: String,

        /// Date of work (YYYY-MM-DD, default: today)
        #[arg(long)]
        date: Option<String>,

        /// Description of work performed
        #[arg(long)]
        notes: Option<String>,

        /// Override billable status
        #[arg(long)]
        billable: Option<bool>,
    },

    /// List time entries with optional filters
    #[command(long_about = "\
List time entries with optional filters.

Returns time entries ordered by date descending. All filters are optional \
and can be combined.

EXAMPLES:
  keito time list --from 2025-01-01 --to 2025-01-31 --json
  keito time list --project acme --limit 10 --json
  keito time list --task dev --page 2 --json")]
    List {
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        from: Option<String>,

        /// End date (YYYY-MM-DD)
        #[arg(long)]
        to: Option<String>,

        /// Filter by project name or ID
        #[arg(long)]
        project: Option<String>,

        /// Filter by task name or ID
        #[arg(long)]
        task: Option<String>,

        /// Max entries to return (default: 50)
        #[arg(long, default_value = "50")]
        limit: u32,

        /// Page number
        #[arg(long, default_value = "1")]
        page: u32,
    },

    /// Show currently running timer
    #[command(long_about = "\
Show the currently running timer, if any.

Returns the active time entry or exit code 4 if no timer is running. \
Agents should call this before `time start` to avoid a conflict.

EXAMPLE:
  $ keito time running --json
  {
    \"id\": \"te_abc123\",
    \"project\": \"Acme Website\",
    \"task\": \"Development\",
    \"started_at\": \"2025-01-15T09:00:00Z\",
    \"is_running\": true,
    \"elapsed\": \"1:23:45\"
  }

EXIT CODES:
  0   Timer is running (entry returned)
  4   No running timer")]
    Running,
}
