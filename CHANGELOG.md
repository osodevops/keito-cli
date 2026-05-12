# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.1.4] - 2026-05-12

### Added

- Client discovery and creation commands for agent setup workflows.
- Project creation from the CLI, including client filtering, billable defaults, explicit task IDs, and conflict handling.
- Agent session recording fields for source metadata, duration-based logging, and setup wizard support.

### Fixed

- Integration tests now write mock config to the Windows `%APPDATA%` path as well as Unix/macOS paths.

## [0.1.3] - 2026-05-05

### Added

- Production API v2 compatibility for `app.keito.ai`, including `/api/v2/users/me`, projects, tasks, time entries, and timer stop support.
- Long-lived API key configuration with account/workspace defaults for agent and human CLI use.
- Recursive man page generation and tests for all agent-facing commands.
- Release gates modeled on `kafka-backup`: version guard, explicit release tag dispatch, pre-release tests, release smoke checks, staged assets, and final CI/release summary jobs.

### Fixed

- Homebrew release smoke test now matches the actual `keito --version` output.
- Production field mapping now uses `account_id`, `spent_date`, nested project/task names, and v2 error envelopes.

## [0.1.2] - 2026-03-05

### Added

- `keito time stop --discard` — abandon a running timer without saving, deletes the time entry
- Richer JSON error output with `suggestion` and `details` fields for agent-friendly recovery hints
- README with install instructions, quick start, agent workflow, and full command reference
- `gen-man` binary for generating man pages (`cargo run --bin gen-man`)
- Agent integration guide at `docs/agent-guide.md`
- VHS demo tape and recording script for terminal demos
- Homepage URL in `--help` output

### Fixed

- Commit `Cargo.lock` so `rustsec/audit-check` can run in CI
- Use `gh release download` for Homebrew tap job (fixes private repo asset downloads)

## [0.1.0] - 2026-03-05

### Added

- `keito auth login` — interactive API key and workspace setup with OS keyring storage
- `keito auth logout` — remove stored credentials from keychain
- `keito auth status` — check authentication status and credential source
- `keito auth whoami` — show current user identity and workspace info
- `keito time start` — start a timer for a project and task
- `keito time stop` — stop the currently running timer
- `keito time log` — log a completed time entry with duration (decimal hours or HH:MM)
- `keito time list` — list time entries with date, project, task, and pagination filters
- `keito time running` — show currently running timer
- `keito projects list` — list available projects in the workspace
- `keito projects show` — show project details by name, code, or ID
- `keito projects tasks` — list workspace-global tasks
- Dual output mode: human-readable tables (TTY default) and JSON (piped default, or `--json`)
- Case-insensitive name/code/ID resolution for projects and tasks
- Exit codes 0-8 per specification (auth, input, conflict, not-found, rate-limit, server, network, config)
- JSON error output with structured `{error, code, message}` format
- Retry logic: 3x exponential backoff (1s, 2s, 4s) for network and server errors
- Credential resolution: `KEITO_API_KEY` env > OS keyring > config file
- Workspace resolution: `--workspace` flag > `KEITO_WORKSPACE_ID` env > config file
- Configuration file at `~/.config/keito/config.toml`
- Rich `--help` documentation with examples, exit codes, agent workflows, and env var reference
- CI pipeline: format, clippy, multi-platform tests, security audit
- Release pipeline: auto-tag on version bump, cross-platform builds, GitHub Releases, Homebrew tap, Scoop bucket
