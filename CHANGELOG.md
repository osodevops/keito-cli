# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `keito time stop --discard` — abandon a running timer without saving, deletes the time entry
- Richer JSON error output with `suggestion` and `details` fields for agent-friendly recovery hints

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
