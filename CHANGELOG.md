# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.1.9] - 2026-09-01

### Fixed

- Render task responses containing currency fields with the task table and its billable column instead of misclassifying them as clients.
- Show the actual human-readable duration format in `time stop`, `time log`, and `time running` help examples.
- Validate agent lifecycle metadata against the 4KB API limit after adding required skill and duration fields.

## [0.1.8] - 2026-09-01

### Added

- Add project-scoped task discovery through `keito projects tasks [PROJECT]` and use embedded/project-filtered tasks for all tracking commands.
- Preserve current API v2 user, company, capability, client, project, task, actual-duration, rounded-duration, internal-note, billability, and external-reference response fields in JSON output.
- Send CLI identity headers and a unique idempotency key for every API mutation, reusing the key across transient retries.
- Include production lifecycle metadata (`skill=keito-time-track` and `duration_seconds`) for agent session records.
- Surface `Retry-After` seconds in structured rate-limit errors.

### Changed

- Reject Personal read-only sync keys during login/status with a clear credential-scope error; `auth whoami` can still display their limited identity response.
- Paginate client, project, and task reference discovery instead of silently stopping after the first page.
- Respect server `Retry-After` delays when retrying transient 5xx responses.
- Use the production safe-delete intent when discarding a running timer and rely exclusively on the server-side timer stop endpoint.
- Accept the current `mobile` and `integration` time-entry source values.

### Fixed

- Parse `/api/v2/users/me` responses that intentionally omit `company` for Personal read-only sync credentials instead of returning a server/serde error.
- Stop sending the removed `is_running` field to the strict time-entry update endpoint during agent session upserts.
- Map production `400`, `413`, and `415` responses to invalid input, and `412`/`428` responses to conflict, while preserving stable CLI exit codes.
- Resolve tasks within the selected project so the CLI no longer offers workspace tasks that production rejects for that project.
- Preserve exact actual seconds and rounded hours in timer/log/session output when returned by the API.
- Return a stable JSON object from `time running` whether or not a timer is active.
- Update `h2` and `quinn-proto` to versions containing their current security fixes.

## [0.1.7] - 2026-06-16

### Added

- Bundle the `keito-time-track` agent skill with the CLI so `keito skill install` no longer depends on an external skill repository by default.
- Add a gstack-style `./setup` entrypoint for source-checkout installs with Claude Code and Codex target selection.
- Add `keito skill team-init optional|required` to write repo-level agent guidance and `.keito/config.example.yml`.
- Add packaged lifecycle hook tests that exercise the bundled skill with a fake Keito CLI.
- Add AI-native services positioning notes for agent billing, project attribution, and client profitability.

### Changed

- Keep the audit-first external skill install path available through `--source` and `--skip-skills-add`.
- Document the bundled skill install, repo setup workflow, and release positioning in README and the agent guide.
- Accept `calendar` and `desktop` as time entry source values alongside `web`, `cli`, `api`, and `agent`.

## [0.1.6] - 2026-05-12

### Changed

- Pin the Agent Skill installer package to `skills@1.5.6` instead of resolving a floating installer tag.
- Clarify CLI help and README wording so the skill remains GitHub-sourced and npm is only used for the pinned installer hop.

## [0.1.5] - 2026-05-12

### Added

- `keito skill install`, `keito skill status`, and `keito skill doctor` commands for installing and verifying the Keito Agent Skill.
- Optional interactive Agent Skill install prompt after `keito auth login`.
- JSON-safe skill installer execution so child installer output does not contaminate `--json` responses.

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
