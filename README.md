# keito

Track billable time against the [Keito](https://keito.ai) platform — from the terminal or from an AI agent.

<p align="center">
  <img src="recordings/time-start-stop-optimised.gif" alt="keito demo" width="720" />
</p>

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

This prompts for your API key (`kto_...`) and account/company ID, validates them against the production v2 API, stores them in `~/.config/keito/config.toml`, and attempts to mirror the API key into the OS keyring. Find the Company ID in Keito under Settings > API & Developers > Company ID.

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

## Features

- **Dual output** — human-readable tables (TTY) or JSON (piped / `--json`)
- **Structured errors** — JSON errors include `suggestion` and `details` fields for agent recovery
- **Exit codes 0–8** — every failure mode has a unique code for programmatic handling
- **Name resolution** — use project names, codes, or IDs interchangeably (case-insensitive)
- **OS keyring** — credentials stored securely via the system keychain
- **Retry logic** — 3× exponential backoff for network and server errors
- **Cross-platform** — macOS, Linux, Windows

## Commands

| Command | Description |
|---|---|
| `keito auth login` | Store API key and configure account/company ID (interactive) |
| `keito auth logout` | Remove stored credentials from keychain |
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

Configuration file: `~/.config/keito/config.toml`

```toml
api_key = "kto_..."
account_id = "co_abc123"
workspace_id = "co_abc123" # legacy alias, kept for compatibility
```

### Credential Precedence

1. `KEITO_API_KEY` environment variable (highest priority)
2. OS keyring (stored by `keito auth login`)
3. `api_key` in config file

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
| `KEITO_API_KEY` | API key — takes precedence over keyring and config |
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
