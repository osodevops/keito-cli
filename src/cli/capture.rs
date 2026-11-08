use clap::{Args, Subcommand, ValueEnum};

#[derive(Args)]
#[command(about = "Capture opted-in coding turns as private, pending-review evidence")]
pub struct CaptureCommand {
    #[command(subcommand)]
    pub command: CaptureSubcommand,
}

#[derive(Subcommand)]
pub enum CaptureSubcommand {
    /// Validate and explicitly approve this repository's mapping on this device
    Setup {
        #[arg(long, value_enum, required = true)]
        agent: Vec<CaptureAgent>,
        #[arg(long)]
        project: String,
        #[arg(long)]
        task: String,
        /// Confirm the displayed metadata-only mapping policy; never bill automatically
        #[arg(long, required = true)]
        confirm: bool,
        /// Install/update this repository's macOS launchd worker (device-only paths)
        #[arg(long)]
        install_worker: bool,
    },
    /// Inspect local consent, queue and delivery state (no network)
    Status,
    /// Reconcile unfinished intervals after a storage-refused boundary, without inventing time
    Recover {
        /// Acknowledge that refused boundaries cannot be reconstructed
        #[arg(long, required = true)]
        confirm: bool,
    },
    /// Inspect minimized evidence for an exact original harness session identifier
    Preview {
        #[arg(long)]
        session: String,
    },
    /// Deliver frozen queued revisions; never fall back to ordinary time writes
    Sync,
    /// Run a bounded supervised foreground sync loop until interrupted
    Worker {
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=60))]
        interval_seconds: u64,
        /// Refuse supervised execution after an unapproved runtime replacement
        #[arg(long)]
        expected_runtime_hash: Option<String>,
    },
    /// Inspect the current repository's macOS worker service without network writes
    WorkerStatus {
        /// Discover integration-owned workers, including stale or removed repositories
        #[arg(long)]
        all: bool,
    },
    /// Stop/remove only this repository's integration-owned launchd service
    WorkerUninstall {
        #[arg(
            long,
            required_unless_present = "worker_id",
            conflicts_with = "worker_id"
        )]
        repo: bool,
        /// Opaque worker ID from worker-status --all; works after repository removal
        #[arg(long, required_unless_present = "repo")]
        worker_id: Option<String>,
    },
    /// Exclude paused intervals and hold this repository's queued deliveries
    Pause {
        #[arg(long, required = true)]
        repo: bool,
    },
    /// Resume this repository after rechecking its approved binding
    Resume {
        #[arg(long, required = true)]
        repo: bool,
    },
    /// Revoke this repository's consent and hold its unsent evidence
    Disable {
        #[arg(long, required = true)]
        repo: bool,
    },
    /// Disable collection and uploads for every approved repository on this device
    DisableAll,
    /// Persist an allowlisted lifecycle signal from bounded JSON on stdin; no network
    Hook {
        #[arg(long, value_enum)]
        agent: CaptureAgent,
        #[arg(long, value_enum)]
        event: CaptureEvent,
        #[arg(long)]
        harness_version: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum CaptureAgent {
    ClaudeCode,
    Codex,
}
impl CaptureAgent {
    pub fn namespace(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }
}
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum CaptureEvent {
    SessionStart,
    Prompt,
    Stop,
    Interrupt,
    SessionEnd,
}
