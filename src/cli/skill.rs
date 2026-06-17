use clap::{Args, Subcommand, ValueEnum};

#[derive(Args)]
#[command(after_long_help = "\
The Keito Skill is the agent UX layer on top of the Keito CLI. The skill
uses the CLI for authentication and API writes, then installs Claude Code /
Codex lifecycle hooks so local coding sessions can be logged automatically.

EXAMPLES:
  keito skill install
  keito skill install --agent codex
  keito skill team-init optional
  keito skill status --json
  keito skill doctor")]
pub struct SkillCommand {
    #[command(subcommand)]
    pub command: SkillSubcommand,
}

#[derive(Subcommand)]
pub enum SkillSubcommand {
    /// Install the Keito Skill and configure supported agent hooks
    #[command(long_about = "\
Install the Keito Skill with the pinned open skills installer package, then run
the installed hook installer for each selected agent.

By default this configures both Codex and Claude Code. The skill still needs
per-repository setup after installation: cd into a client repo and run
/track-time-keito to select its Keito client, project, and task.")]
    Install {
        /// Skill source for the skills installer, or "bundled" for the skill shipped with this CLI
        #[arg(long, default_value = "bundled", env = "KEITO_SKILL_SOURCE")]
        source: String,

        /// Agent hook target to configure
        #[arg(long, value_enum)]
        agent: Vec<SkillAgent>,

        /// Skip the skills installer and only run hook installers if present
        #[arg(long)]
        skip_skills_add: bool,
    },

    /// Show install/auth/hook status for the Keito Skill
    Status,

    /// Run readiness checks and print next actions
    Doctor,

    /// Add repo-level Keito tracking guidance for agent teammates
    #[command(long_about = "\
Add Keito tracking guidance to the current Git repository.

This mirrors gstack-style team mode: the Keito skill remains globally installed,
while the repository records how agents should set up local project/task mapping.
It updates AGENTS.md for Codex/OpenAI agents, CLAUDE.md for Claude Code, and
.gitignore for the repo-local .keito/config.yml file.

Use \"optional\" to suggest tracking, or \"required\" to tell agents to stop
before billable coding work until /track-time-keito has configured the repo.")]
    TeamInit {
        /// Team policy to write into repo guidance
        #[arg(value_enum)]
        mode: SkillTeamMode,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SkillAgent {
    Codex,
    ClaudeCode,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SkillTeamMode {
    Optional,
    Required,
}

impl SkillAgent {
    pub fn skills_cli_name(self) -> &'static str {
        match self {
            SkillAgent::Codex => "codex",
            SkillAgent::ClaudeCode => "claude-code",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            SkillAgent::Codex => "Codex",
            SkillAgent::ClaudeCode => "Claude Code",
        }
    }
}
