# Keito CLI - AI Agent Time Tracking and Billing

[![CI](https://github.com/osodevops/keito-cli/actions/workflows/test.yml/badge.svg)](https://github.com/osodevops/keito-cli/actions/workflows/test.yml)

Track billable human and AI agent work in [Keito](https://keito.ai) from the terminal, CI, scripts, or autonomous agent workflows.

`keito` is a command-line interface for AI agencies, professional services teams, and AI-native service companies that need billing-grade records for client work. Use it to start and stop timers, log completed work, map time to Keito projects and tasks, and return structured JSON that agents can safely use without scraping terminal text.

Keito is built for teams selling outcomes with people and AI agents. The CLI helps make that work visible for client billing, project profitability, time tracking, and invoice review.

Keito is billing and profitability infrastructure for AI-native services teams:
agent work is recorded against the same clients, projects, tasks, and invoices
as human work, with metadata for margin and audit analysis.

<p align="center">
  <img src="recordings/time-start-stop-optimised.gif" alt="keito demo" width="720" />
</p>

## Why Keito CLI?

- **AI agent time tracking** - record billable work from coding agents, automation agents, and human operators in the same Keito workspace.
- **Client billing for AI work** - attach work to projects and tasks so AI-assisted delivery can be reviewed before invoicing.
- **Built for AI-native service companies** - support agency, consulting, and professional services teams where delivery is a mix of humans, agents, and automated workflows.
- **Agent-safe automation** - every command supports JSON output, stable exit codes, explicit errors, and non-interactive execution.
- **Project margin visibility** - keep service delivery records in Keito so time, billing, and profitability workflows have reliable source data.

Related Keito pages:

- [AI agent billing for agencies and developers](https://keito.ai/agents)
- [AI agency billing software](https://keito.ai/solutions/ai-agent-cost-tracking/ai-agency-billing-software/)
- [Billing for AI-native service companies](https://keito.ai/solutions/ai-agent-cost-tracking/ai-native-service-company-billing/)
- [Professional services AI agent billing](https://keito.ai/solutions/billing/professional-services-ai-agent-billing/)

## Install

### Homebrew (macOS / Linux)

```sh
brew install osodevops/tap/keito
```

### GitHub Releases

Download the latest binary from [Releases](https://github.com/osodevops/keito-cli/releases) for your platform (macOS, Linux, Windows).

### Build from Source

```sh
git clone https://github.com/osodevops/keito-cli.git
cd keito-cli
cargo build --release
# binary at target/release/keito
```

Requires Rust 1.75+.

## Setup

```sh
keito auth login
```

This prompts for your API key (`kto_...`) and account/company ID, validates them against the production v2 API, and stores them in the platform config file. On macOS this is `~/Library/Application Support/keito/config.toml`; on Linux this is typically `~/.config/keito/config.toml`. Find the Company ID in Keito under Settings > API & Developers > Company ID.

For agent / CI use, set environment variables instead:

```sh
export KEITO_API_KEY=kto_xxx
export KEITO_ACCOUNT_ID=co_abc123
```

## Quick Start

```sh
# Verify credentials
keito auth whoami

# Browse projects and tasks
keito projects list
keito projects tasks

# Start a timer
keito time start --project "Acme Website" --task dev

# Check what's running
keito time running

# Stop when done
keito time stop

# Log time after the fact
keito time log --project acme --task dev --duration 1:30 --notes "Fixed auth bug"
```

## Agent Quick Start

Every command supports `--json` output and returns structured exit codes for programmatic error handling:

```sh
# Verify credentials
keito auth status --json

# Discover projects and tasks
keito projects list --json
keito projects tasks --json

# Start → work → stop
keito time start --project "Acme Website" --task dev --json
keito time running --json
keito time stop --json
```

Exit codes tell you exactly what happened — no need to parse error messages. See [Exit Codes](#exit-codes) below.

## Agent Skill

The Keito Agent Skill ships with this CLI. It brings Keito to the places where
agents already work by installing lifecycle hooks for Claude Code and OpenAI
Codex CLI, then recording one `source=agent` time entry when a tracked coding
session ends.

### Requirements

- Claude Code and/or Codex CLI
- Git, Bash, and `jq`
- Keito credentials via `keito auth login` or `KEITO_API_KEY` +
  `KEITO_ACCOUNT_ID`
- macOS/Linux: `./setup` can install the CLI through Homebrew or release
  tarballs if `keito` is not already on `PATH`
- Windows: install the binary from
  [Releases](https://github.com/osodevops/keito-cli/releases) first, then run
  the skill commands from Git Bash or WSL where `bash` and `jq` are available

### Ask Your Agent to Install Keito

Keito is designed to be installed by the coding agent itself. Paste the
relevant prompt into the agent you use.

Claude Code:

> Install Keito for Claude Code: run **`git clone --single-branch --depth 1 https://github.com/osodevops/keito-cli.git ~/.keito/keito-cli && cd ~/.keito/keito-cli && ./setup --host claude`**. Then run `keito skill doctor`. If Keito is unauthenticated, stop and tell me to run `keito auth login` or set `KEITO_API_KEY` and `KEITO_ACCOUNT_ID`; never ask for or print API keys. After install, ask whether I want to enable tracking in the current repo with `/track-time-keito`, and whether to add team guidance with `keito skill team-init optional` or `keito skill team-init required`.

Codex:

> Install Keito for Codex: run **`git clone --single-branch --depth 1 https://github.com/osodevops/keito-cli.git ~/.keito/keito-cli && cd ~/.keito/keito-cli && ./setup --host codex`**. Then run `keito skill doctor`. If Keito is unauthenticated, stop and tell me to run `keito auth login` or set `KEITO_API_KEY` and `KEITO_ACCOUNT_ID`; never ask for or print API keys. After install, ask whether I want to enable tracking in the current repo with `/track-time-keito`, and whether to add team guidance with `keito skill team-init optional` or `keito skill team-init required`.

Other AI coding agents:

> Install Keito agent time tracking: run **`git clone --single-branch --depth 1 https://github.com/osodevops/keito-cli.git ~/.keito/keito-cli && cd ~/.keito/keito-cli && ./setup --host both`**. Then run `keito skill doctor`. If this agent does not support Keito lifecycle hooks, use the Keito CLI directly with `--json` commands and add repo guidance to `AGENTS.md` that says billable agent work should be tracked through Keito.

### Step 1: Install on your machine

Open Claude Code or Codex and paste this. The agent can run the same
source-checkout flow that gstack documents:

```sh
git clone --single-branch --depth 1 https://github.com/osodevops/keito-cli.git ~/.keito/keito-cli
cd ~/.keito/keito-cli
./setup
```

`./setup` installs the CLI if needed, installs the bundled `keito-time-track`
skill, and configures hooks for Claude Code, Codex, or both. In an interactive
terminal it asks which host to configure. In non-interactive runs it defaults
to both.

Target one host explicitly when needed:

```sh
~/.keito/keito-cli/setup --host claude
~/.keito/keito-cli/setup --host codex
~/.keito/keito-cli/setup --host both
```

If the CLI is already installed, this equivalent command uses the bundled skill
without the source checkout:

```sh
keito skill install
```

Choose one target when needed:

```sh
keito skill install --agent claude-code
keito skill install --agent codex
```

Check readiness:

```sh
keito skill doctor
keito skill status --json
```

### Step 2: Configure each client repo

From each client repository, invoke the skill once to map that worktree to a
Keito client, project, and task:

```text
/track-time-keito
```

This writes `.keito/config.yml`, which is intentionally repo-local and should
not be committed.

### Team Mode

For shared repos, use the same model as gstack team mode: the skill remains
globally installed, and the repository commits only agent guidance plus an
example config.

From inside the shared repo:

```sh
~/.keito/keito-cli/setup --team optional
git add AGENTS.md CLAUDE.md .gitignore .keito/config.example.yml
git commit -m "add Keito tracking guidance for agent work"
```

Use `required` instead of `optional` when agents must stop before billable
coding work until `/track-time-keito` has configured the repo:

```sh
~/.keito/keito-cli/setup --team required
```

If the CLI is already installed and the global skill is already configured, you
can run only the repo bootstrap:

```sh
keito skill team-init optional
keito skill team-init required
```

`team-init` writes `AGENTS.md`, `CLAUDE.md`, `.gitignore`, and
`.keito/config.example.yml`. It does not vendor the skill into the repo. Do not
commit `.keito/config.yml`; that file contains the local project/task mapping
created by `/track-time-keito`.

### How It Works

- `./setup` is the gstack-style source-checkout installer. It keeps a stable
  checkout at `~/.keito/keito-cli`, installs the CLI if needed, then calls
  `keito skill install --source bundled`.
- `keito skill install` materializes the bundled `keito-time-track` skill and
  copies it into agent home directories:
  `~/.claude/skills/keito-time-track` for Claude Code and
  `~/.codex/skills/keito-time-track` for Codex.
- The hook installers merge lifecycle hooks into `~/.claude/settings.json` and
  `~/.codex/hooks.json` using `jq`.
- `/track-time-keito` verifies auth, asks for the Keito client/project/task for
  the current repo, and writes `.keito/config.yml`.
- On agent session start, the hook records local session state. On session end,
  the hook logs one Keito time entry with `source=agent` and metadata such as
  repo path, branch, commit, and agent type.

### Distribution Model

The default install path is Git-based and auditable: clone this public repo,
run `./setup`, and install the skill copy bundled with the reviewed CLI
release. That gives agents a deterministic install path without making npm the
primary trust boundary.

`npx` remains useful as a convenience for audit-first external installs, but it
is not required for the bundled path. When using `npx`, pin the installer
version exactly, inspect the source repo, and avoid `latest` in automation.

Audit-first external install remains available:

```sh
npx --yes skills@1.5.6 add osodevops/keito-skill -g -a codex -a claude-code -s keito-time-track -y --copy
keito skill install --skip-skills-add
```

## Features

- **Dual output** — human-readable tables (TTY) or JSON (piped / `--json`)
- **Structured errors** — JSON errors include `suggestion` and `details` fields for agent recovery
- **Exit codes 0–8** — every failure mode has a unique code for programmatic handling
- **Name resolution** — use project names, codes, or IDs interchangeably (case-insensitive)
- **Config-backed auth** — long-lived API keys are stored in a local `config.toml` for agent-friendly execution
- **Retry logic** — 3× exponential backoff for network and server errors
- **Cross-platform** — macOS, Linux, Windows

## Commands

| Command | Description |
|---|---|
| `keito auth login` | Store API key and configure account/company ID (interactive) |
| `keito auth logout` | Remove stored credentials from config |
| `keito auth status` | Check authentication status and credential source |
| `keito auth whoami` | Show current user identity and account info |
| `keito time start` | Start a timer for a project and task |
| `keito time stop` | Stop the currently running timer |
| `keito time log` | Log a completed time entry with duration |
| `keito time list` | List time entries with optional filters |
| `keito time running` | Show the currently running timer |
| `keito projects list` | List available projects in the workspace |
| `keito projects show` | Show project details by name, code, or ID |
| `keito projects tasks` | List tasks (global, not per-project) |

Run `keito <command> --help` for detailed usage, examples, and exit codes.

## Configuration

Configuration file:

- macOS: `~/Library/Application Support/keito/config.toml`
- Linux: `~/.config/keito/config.toml`
- Windows: `%APPDATA%\\keito\\config.toml`

```toml
api_key = "kto_..."
account_id = "co_abc123"
workspace_id = "co_abc123" # legacy alias, kept for compatibility
api_url = "https://app.keito.ai"
```

### Credential Precedence

1. `KEITO_API_KEY` environment variable (highest priority)
2. `api_key` in config file

### Account ID Precedence

Find the Company ID in Keito under Settings > API & Developers > Company ID.

1. `--workspace` CLI flag
2. `KEITO_ACCOUNT_ID` environment variable
3. `KEITO_WORKSPACE_ID` environment variable (legacy alias)
4. `account_id` in config file
5. `workspace_id` in config file (legacy alias)

## Environment Variables

| Variable | Description |
|---|---|
| `KEITO_API_KEY` | API key — takes precedence over config |
| `KEITO_API_URL` | API base URL override; defaults to `https://app.keito.ai` |
| `KEITO_ACCOUNT_ID` | Company/account ID sent as `Keito-Account-Id` |
| `KEITO_WORKSPACE_ID` | Legacy alias for `KEITO_ACCOUNT_ID` |

## Output Formats

### Table (default for TTY)

```
$ keito time list --limit 2
 Date       | Project       | Task        | Duration | Billable
------------+---------------+-------------+----------+---------
 2025-01-15 | Acme Website  | Development | 1:30     | Yes
 2025-01-14 | Acme Website  | Design      | 2:00     | Yes
```

### JSON (default when piped, or `--json`)

```json
[
  {
    "id": "te_abc123",
    "project": "Acme Website",
    "task": "Development",
    "duration": 1.5,
    "date": "2025-01-15",
    "billable": true
  }
]
```

## Exit Codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Authentication error (missing or invalid API key) |
| 2 | Invalid input (bad arguments, malformed duration) |
| 3 | Conflict (e.g. timer already running) |
| 4 | Not found (project, task, or entry does not exist) |
| 5 | Rate limited (retry after a moment) |
| 6 | Server error (Keito API 5xx) |
| 7 | Network error (connection failed, timeout) |
| 8 | Configuration error (missing config, bad TOML) |

## Building from Source

```sh
git clone https://github.com/osodevops/keito-cli.git
cd keito-cli
cargo build --release
cargo test --all-targets
```

### Generate Man Pages

```sh
cargo run --bin gen-man
man ./man/keito.1
```

## License

[MIT](LICENSE)
