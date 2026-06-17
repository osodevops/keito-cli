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
and --task flags accept a name, code, or ID (case-insensitive).

API EFFECT:
  1. GET /api/v2/time_entries?is_running=true
  2. POST /api/v2/time_entries with spent_date, is_running=true, source=cli

EXAMPLE:
  $ keito time start --project \"Acme Website\" --task dev --json
  {
    \"status\": \"started\",
    \"entry_id\": \"te_abc123\",
    \"project\": \"Acme Website\",
    \"task\": \"Development\",
    \"spent_date\": \"2026-03-04\",
    \"billable\": true,
    \"source\": \"cli\",
    \"started_at\": \"2026-03-04T09:00:00Z\"
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

Stops the active running time entry through the production stop endpoint. \
The API calculates elapsed duration server-side to avoid client clock races. \
When --notes is supplied, notes replace the existing entry notes. Without \
--notes, existing notes are preserved. Use --discard to delete the running \
timer instead of saving it.

API EFFECT:
  1. GET /api/v2/time_entries?is_running=true
  2. PATCH /api/v2/time_entries/{id}/stop
     or DELETE /api/v2/time_entries/{id} when --discard is supplied

EXAMPLE:
  $ keito time stop --json
  {
    \"status\": \"stopped\",
    \"entry_id\": \"te_abc123\",
    \"project\": \"Acme Website\",
    \"task\": \"Development\",
    \"duration_hours\": 1.5,
    \"duration\": \"1:30\",
    \"spent_date\": \"2026-03-04\",
    \"billable\": true,
    \"source\": \"cli\"
  }

EXIT CODES:
  0   Timer stopped (or discarded)
  3   Conflict — timer is no longer running
  4   No running timer found")]
    Stop {
        /// Replace notes on the entry
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
  --duration-seconds 5400 → 1 hour 30 minutes

EXAMPLE:
  $ keito time log --project acme --task dev --duration 1:30 \\
      --date 2025-01-15 --notes \"Fixed auth bug\" --json
  {
    \"status\": \"logged\",
    \"entry_id\": \"te_def456\",
    \"project\": \"Acme Website\",
    \"task\": \"Development\",
    \"duration_hours\": 1.5,
    \"duration\": \"1:30\",
    \"spent_date\": \"2025-01-15\",
    \"date\": \"2025-01-15\",
    \"billable\": true,
    \"source\": \"cli\"
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
        duration: Option<String>,

        /// Duration in whole seconds
        #[arg(long = "duration-seconds", conflicts_with = "duration")]
        duration_seconds: Option<u64>,

        /// Date of work (YYYY-MM-DD, default: today)
        #[arg(long)]
        date: Option<String>,

        /// Start time of work (HH:MM)
        #[arg(long = "started-time")]
        started_time: Option<String>,

        /// End time of work (HH:MM)
        #[arg(long = "ended-time")]
        ended_time: Option<String>,

        /// Description of work performed
        #[arg(long)]
        notes: Option<String>,

        /// Override billable status
        #[arg(long)]
        billable: Option<bool>,

        /// Source to store on the time entry: web, cli, api, agent, calendar, or desktop
        #[arg(long, default_value = "cli")]
        source: String,

        /// JSON metadata object to store on the time entry
        #[arg(long)]
        metadata: Option<String>,

        /// Agent session ID to store in metadata.session_id
        #[arg(long = "session-id")]
        session_id: Option<String>,

        /// Agent identifier to store in metadata.agent_id
        #[arg(long = "agent-id")]
        agent_id: Option<String>,

        /// Agent type to store in metadata.agent_type
        #[arg(long = "agent-type")]
        agent_type: Option<String>,

        /// Skill name to store in metadata.skill
        #[arg(long)]
        skill: Option<String>,
    },

    /// Create or update a completed agent session entry
    #[command(long_about = "\
Create or update a completed agent session time entry.

This is intended for agent lifecycle hooks. The command records a completed \
entry with source=agent by default and metadata.session_id set to the supplied \
session ID. If an entry for the same session ID already exists on the target \
date/source, it is updated instead of duplicated.

EXAMPLE:
  keito time session-record --project acme --task dev \\
    --session-id codex-123 --duration-seconds 5400 \\
    --started-at 2026-05-11T09:00:00Z --ended-at 2026-05-11T10:30:00Z \\
    --skill keito-agent --json")]
    SessionRecord {
        /// Project name, code, or ID
        #[arg(long)]
        project: String,

        /// Task name or ID
        #[arg(long)]
        task: String,

        /// Stable agent session ID used for idempotent upsert behavior
        #[arg(long = "session-id")]
        session_id: String,

        /// Duration in whole seconds
        #[arg(long = "duration-seconds")]
        duration_seconds: u64,

        /// RFC3339 session start timestamp
        #[arg(long = "started-at")]
        started_at: Option<String>,

        /// RFC3339 session end timestamp
        #[arg(long = "ended-at")]
        ended_at: Option<String>,

        /// Date of work (YYYY-MM-DD, default: local date from started-at or today)
        #[arg(long)]
        date: Option<String>,

        /// Description of work performed
        #[arg(long)]
        notes: Option<String>,

        /// Override billable status
        #[arg(long)]
        billable: Option<bool>,

        /// Source to store on the time entry: web, cli, api, agent, calendar, or desktop
        #[arg(long, default_value = "agent")]
        source: String,

        /// JSON metadata object to store on the time entry
        #[arg(long)]
        metadata: Option<String>,

        /// Agent identifier to store in metadata.agent_id
        #[arg(long = "agent-id")]
        agent_id: Option<String>,

        /// Agent type to store in metadata.agent_type
        #[arg(long = "agent-type")]
        agent_type: Option<String>,

        /// Skill name to store in metadata.skill
        #[arg(long)]
        skill: Option<String>,
    },

    /// List time entries with optional filters
    #[command(long_about = "\
List time entries with optional filters.

Returns time entries ordered by date descending. All filters are optional \
and can be combined. JSON output uses production v2 field names such as \
spent_date, billable, source, project, and task.

API EFFECT:
  GET /api/v2/time_entries with page, per_page, and optional filters

EXAMPLES:
  keito time list --from 2025-01-01 --to 2025-01-31 --json
  keito time list --today --source agent --json
  keito time list --project acme --limit 10 --json
  keito time list --task dev --page 2 --json")]
    List {
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        from: Option<String>,

        /// End date (YYYY-MM-DD)
        #[arg(long)]
        to: Option<String>,

        /// Use today's local date for both --from and --to
        #[arg(long)]
        today: bool,

        /// Filter by project name or ID
        #[arg(long)]
        project: Option<String>,

        /// Filter by task name or ID
        #[arg(long)]
        task: Option<String>,

        /// Filter by source: web, cli, api, agent, calendar, or desktop
        #[arg(long)]
        source: Option<String>,

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

Returns the active time entry summary, or {\"running\": false} if no timer is \
running. Agents should call this before `time start` to avoid a conflict.

API EFFECT:
  GET /api/v2/time_entries?is_running=true

EXAMPLE:
  $ keito time running --json
  [
    {
      \"running\": true,
      \"entry_id\": \"te_abc123\",
      \"project\": \"Acme Website\",
      \"task\": \"Development\",
      \"spent_date\": \"2026-03-04\",
      \"billable\": true,
      \"source\": \"cli\",
      \"started_at\": \"2026-03-04T09:00:00Z\",
      \"elapsed_hours\": 1.5,
      \"elapsed\": \"1:30\"
    }
  ]

EXIT CODES:
  0   Command succeeded; inspect JSON running field")]
    Running,
}
