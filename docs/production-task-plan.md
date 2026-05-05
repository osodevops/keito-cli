# Keito CLI Production Task Plan

Date: 2026-05-05

This is the implementation task plan to get `keito-cli` production-ready. It combines:

- the local CLI PRD and man pages in this repo,
- the current production app code in `/Users/sionsmith/development/oso/com.github.osodevops/keito`,
- the public Keito developer docs currently visible at `keito.ai/docs`,
- live unauthenticated probes against `app.keito.ai`,
- Rust CLI patterns from sibling repos under `/Users/sionsmith/development/oso/com.github.osodevops`.

## Source Findings

### CLI Repo

Current implemented surface:

- `keito auth login|logout|status|whoami`
- `keito projects list|show|tasks`
- `keito time start|stop|log|list|running`
- top-level generated man pages under `man/`

Current missing PRD/public-doc surface:

- `keito time edit|delete|status`
- `keito expense log|list`
- `keito clients list|show`
- `keito reports summary|entries`
- `keito config show|set|init|list`
- `keito completions <shell>`
- agent metadata flags and auto-detection
- offline queue and `sync`
- full subcommand man pages

### Production App Contract

The production app repo currently implements API v2 under `src/app/api/v2/`.

Observed live probes on 2026-05-05:

- `GET https://app.keito.ai/api/health` returns healthy JSON.
- `GET https://app.keito.ai/api/v2/users/me` returns v2 JSON `401` when unauthenticated.
- `GET https://app.keito.ai/api/v2/projects` returns v2 JSON `401` when unauthenticated.

Code/docs contract from the app repo:

- Base URL: `https://app.keito.ai/api/v2`
- Auth: `Authorization: Bearer <kto_...>` plus `Keito-Account-Id: <company_cuid>` for API keys.
- Alternate auth: WorkOS JWT without `Keito-Account-Id`.
- List envelopes use endpoint keys: `projects`, `tasks`, `time_entries`, `expenses`, `clients`, etc.
- Time entry create requires `project_id`, `task_id`, `spent_date`.
- Time entry billable field is `billable`.
- Responses use nested `project` and `task` objects plus `spent_date`.
- `source` and `metadata` are implemented on time entries and expenses.
- Metadata max size is 4KB.
- Existing `PATCH /api/v2/time_entries/{id}` does not currently accept or apply `is_running`, so CLI timer stop needs an API change or a dedicated stop endpoint.

### Public Keito Docs Contract

Public developer docs currently show a different contract:

- Base URL: `https://api.keito.ai/v1`
- Paths like `/v1/time-entries`
- Cursor pagination with `data`, `next_cursor`, `has_more`
- `is_billable` in request/response examples
- CLI docs include `time delete`, `expense log`, `config list`, `agent-mode`, and `--agent-id`/`--session-id`

This conflict must be resolved before GA. The CLI should target one contract, and docs/app/API should agree.

Decision for this CLI: target the real production v2 API (`https://app.keito.ai/api/v2`). Treat the public `/v1` docs as stale/future documentation until platform engineering ships a production-backed `/v1`.

## Rust CLI Best-Practice Notes From Sibling Repos

Use these patterns:

- From `todoist-agent-cli`: workspace split into API/cache/CLI crates, `clap_complete` completions command, `xtask` man-page generation for top-level and nested subcommands.
- From `wordpress-cli`: shared `RenderPayload` + `OutputConfig`, `OutputFormat::Auto`, field masks, NDJSON for large output, global `--no-prompt`, `--dry-run`, `--all-pages`, and command-generated man pages.
- From `xero-cli`: typed `OutputFormat`, `--all-pages`, `--page-size`, `--modified-since`, semantic error diagnostics, Retry-After aware backoff, explicit API budget/rate-limit handling, fixtures for API tests.
- From `k2i`: explicit CLI exit-code enum and structured logging format choices.

Use official clap patterns:

- Keep `clap` derive with reusable `#[command(flatten)]` global flags and `ValueEnum` for constrained values.
- Generate shell completions at runtime with `clap_complete`.
- Generate man pages from `clap::Command`, ideally via an `xtask` or generator binary that walks subcommands recursively.

## Decision Gates

### D1: API Version and Host

Owner: platform + CLI

Decision: CLI targets current production v2: `https://app.keito.ai/api/v2`.

Task outcome:

- Done in this repo: client default is `https://app.keito.ai`, auth identity uses `GET /api/v2/users/me`, and `README.md`, `docs/agent-guide.md`, and generated man pages have been updated.
- File platform docs tasks to make public docs stop advertising the incompatible `/v1` contract.
- Add a smoke-test script for v2 health/auth endpoints.

### D2: Timer Stop API

Owner: platform + CLI

Choose one:

- **Preferred:** Add `PATCH /api/v2/time_entries/{id}/stop`, matching Harvest semantics.
- Or make `PATCH /api/v2/time_entries/{id}` accept `is_running=false` and route to `TimeEntryRepository.stopTimer`.

Task outcome:

- CLI `keito time stop` stops a timer without client-side duration races.
- API docs and OpenAPI include the stop behavior.

### D3: CLI Package Shape

Owner: CLI

Choose one:

- Keep single crate for speed now.
- Split later into `keito-api`, `keito-cli`, `keito-output`, `keito-config`.

Recommendation: keep single crate for P0 compatibility, but organize modules so a later split is mechanical.

## Next Phase: Timer Stop and Agent-Grade Man Pages

This phase has been implemented locally across the CLI repo and the sibling production app repo. It should still be treated as a single release slice because `time stop` is part of the core agent workflow and the man pages are the agent-facing command contract.

### Scope

1. Done: resolve the production timer-stop API contract.
2. Done: update the CLI to stop timers through the chosen production API path.
3. Done: make generated man pages complete enough for an agent to discover and run every supported command without reading README or source.
4. Done: lock the behavior with production-shaped tests and generated-doc checks.

### Recommended API Decision

Implement a production endpoint matching Harvest semantics:

`PATCH /api/v2/time_entries/{id}/stop`

Platform app tasks in `/Users/sionsmith/development/oso/com.github.osodevops/keito`:

- Done: add `src/app/api/v2/time_entries/[id]/stop/route.ts`.
- Done: reuse `validateApiRequest`, company scoping, owner/manager checks, locked-entry checks, and approved-entry conflict behavior from `time_entries/[id]/route.ts`.
- Done: call `TimeEntryRepository.stopTimer(id)` so elapsed hours are calculated server-side.
- Done: accept an optional JSON body with `notes`.
- Done: notes replace existing notes only when `notes` is provided; otherwise existing notes are preserved.
- Done: re-fetch with `user`, `project`, and `task` includes and return `transformTimeEntry`.
- Done: return `404` when the entry does not exist or is outside the company.
- Done: return `409` when the entry is not running.
- Done: add app-side route tests for success, not running, forbidden, and bad input.
- Done: update the platform OpenAPI docs for the stop endpoint.

CLI tasks:

- Done: add `KeitorClient::stop_time_entry(id, notes)` that calls `PATCH /api/v2/time_entries/{id}/stop`.
- Done: change `keito time stop` to use `stop_time_entry` instead of client-side elapsed-hour calculation plus `PATCH is_running=false`.
- Done: keep `--discard` using `DELETE /api/v2/time_entries/{id}`.
- Done: keep JSON output stable:
  - `status`
  - `entry_id`
  - `project`
  - `task`
  - `duration_hours`
  - `duration`
  - `started_at`
  - `stopped_at` if production returns enough information; otherwise omit.
- Done: add production-shaped fixtures:
  - `time_entry_running.json`
  - `time_entry_stopped.json`
  - `error_409.json`
- Done: update `tests/api_mock.rs` to assert:
  - `GET /api/v2/time_entries?is_running=true&per_page=200` before stop;
  - `PATCH /api/v2/time_entries/{id}/stop` for save;
  - optional `notes` body only when notes are supplied;
  - `DELETE /api/v2/time_entries/{id}` for discard;
  - no `is_running=false` request body is sent by the CLI stop path.

### Man Page Requirements

Man pages are now generated recursively, not only for top-level command groups.

Required generated pages:

- `keito.1`
- `keito-auth.1`
- `keito-auth-login.1`
- `keito-auth-logout.1`
- `keito-auth-status.1`
- `keito-auth-whoami.1`
- `keito-projects.1`
- `keito-projects-list.1`
- `keito-projects-show.1`
- `keito-projects-tasks.1`
- `keito-time.1`
- `keito-time-start.1`
- `keito-time-stop.1`
- `keito-time-log.1`
- `keito-time-list.1`
- `keito-time-running.1`

Generator tasks:

- Done: update `src/bin/gen-man.rs` to walk the clap command tree recursively.
- Done: preserve full command paths in page names: `keito-time-start.1`, not just `start.1`.
- Done: make cross references point only to pages that are actually generated.
- Done: add a test that runs the generator into a temp directory and asserts all expected pages exist and are non-empty.
- Done: add generated-page checks for stale API fields:
  - no `/api/v2/me`;
  - no `app.keito.io`;
  - no `is_billable` for create/log examples;
  - no `date` request-field language where `spent_date` is meant;
  - no missing referenced pages like `keito-time-start(1)`.

Clap/help content tasks:

- Every command and subcommand must have:
  - one-line purpose;
  - production API effect;
  - required credentials/env/config inputs;
  - all flags and defaults;
  - JSON output example;
  - table/human behavior summary;
  - exit codes;
  - agent workflow notes when relevant;
  - recovery suggestions for common failures.
- `keito time stop` man page must explicitly document the production stop endpoint behavior, `--discard`, notes behavior, no-running-timer behavior, and exit codes.
- `keito time start/log/list/running` man pages must document production v2 field names in JSON examples: `spent_date`, `billable`, `source`, and nested project/task-derived display values where applicable.
- Regenerate man pages with `cargo run --bin gen-man` after help updates.

### Acceptance Criteria

- `keito time stop --json` works against production-shaped mocks without client-side duration races.
- `time stop` no longer sends `is_running=false` to `PATCH /api/v2/time_entries/{id}`.
- `cargo test --all-targets` passes in `keito-cli`.
- `cargo clippy --all-targets -- -D warnings` passes in `keito-cli`.
- Platform app route tests for the stop endpoint pass.
- Live production smoke test passes for auth, project/task discovery, timer start, running status, duplicate-start conflict, stop, discard, no-running stop handling, manual log, and list.
- `cargo run --bin gen-man` emits every required command and subcommand man page.
- Man-page completeness checks pass.
- README, `docs/agent-guide.md`, `AGENTS.md`, and `docs/production-task-plan.md` agree with the generated man pages.

## P0 Task List: Production Compatibility

### P0.1 Normalize API Base and Auth

- Done: change default API base URL to the chosen canonical production URL.
- Done: rename docs to explain `KEITO_ACCOUNT_ID` is the company/account ID used in `Keito-Account-Id`.
- Done: keep env compatibility with `KEITO_WORKSPACE_ID` and add `KEITO_ACCOUNT_ID` alias.
- Done: update `auth whoami` to call production `GET /api/v2/users/me`.
- Done: update `MeResponse` to production user/company shape.
- Done: support long-lived config-file `api_key` and `account_id`, with `workspace_id` retained as a legacy alias.
- Make `auth status --json` return non-zero on invalid credentials when explicitly asked to validate, or document the existing behavior if retained.
- Add tests for missing token, missing account ID, invalid API key, and config-file credential resolution.

### P0.2 Replace Mock API Contract With Production Fixtures

- Done: add production-shaped fixtures under `tests/fixtures/api_v2/`:
  - `users_me.json`
  - `projects_list.json`
  - `tasks_list.json`
  - `time_entries_list.json`
  - `time_entry_create.json`
  - `error_401.json`
  - `error_400.json`
  - `error_404.json`
  - `error_429.json`
- Later: add `expense_create.json` and `error_409.json` when expense/edit paths are implemented.
- Done: rewrite `tests/api_mock.rs` to assert:
  - endpoint-key envelopes, not `data`;
  - `spent_date`, not `date`;
  - nested `project`/`task`;
  - `billable`, not `is_billable`;
  - `source` and `metadata`.
- Done: model/unit tests fail if production-shaped time-entry fixtures cannot deserialize or request serialization regresses.

### P0.3 Update API Models and Client

- Done: replace generic `PaginatedResponse<T> { data }` with keyed envelopes:
  - `ProjectsEnvelope { projects, page, per_page, total_pages, total_entries, links }`
  - `TasksEnvelope { tasks, ... }`
  - `TimeEntriesEnvelope { time_entries, ... }`
  - Later: `ExpensesEnvelope { expenses, ... }`
- Done: add shared links struct for keyed envelopes.
- Done: update `Project` to use nested `client`.
- Done: update `Task` to use `billable_by_default`.
- Done: update `TimeEntry` to use:
  - `spent_date`
  - nested `user`, `project`, `task`
  - `billable`
  - `source`
  - `metadata`
  - `started_time`, `ended_time`, `timer_started_at`
- Done: update create request structs to use production field names; update request keeps a temporary `is_running` field until timer-stop API is resolved.
- Use URL query builders instead of manual query string concatenation.
- Add `User-Agent: keito-cli/<version>`.

### P0.4 Make Existing Time Commands Work

- `time start`
  - Done: resolve project and task against production list envelopes.
  - Done: send `spent_date`, `is_running=true`, `source=cli`, and optional `metadata`.
  - Later: default source to `agent` when agent context is detected.
  - Done: avoid creating a second timer if one is already running.
- `time running`
  - Query `is_running=true`.
  - Return one object in JSON when one timer is running; return `{"running": false}` and the documented exit code when none.
  - Decide whether no-running-timer is success or exit 4; docs and tests must match.
- `time stop`
  - Use chosen stop API.
  - Support `--notes`, `--discard`, and later `--round`.
  - Preserve/merge notes according to documented behavior.
- `time log`
  - Rename `--duration` to keep backward compatibility, but add `--hours` alias because public docs use `--hours`.
  - Done: send `spent_date`, `hours`, `billable`, `source=cli`, and optional `metadata`.
  - Validate max 24 hours and positive duration.
- `time list`
  - Done: use `page` and `per_page`.
  - Add filters: `--source`, `--agent-id` if supported by API or client-side metadata filter.

### P0.5 Add Agent Metadata Support

- Add value enum for `source`: `web`, `cli`, `api`, `agent`.
- Add flags to `time start` and `time log`:
  - `--source`
  - `--agent-id`
  - `--session-id`
  - `--agent-type`
  - `--model`
  - `--metadata <JSON>`
- Auto-detect:
  - `CLAUDE_SESSION_ID` -> `agent_type=claude-code`
  - `CODEX_SESSION_ID` -> `agent_type=codex`
  - `OPENCLAW_AGENT_ID` -> `agent_type=openclaw`
  - `CURSOR_AGENT` -> `agent_type=cursor`
- Validate metadata is an object and under 4KB before sending.
- Merge explicit `--metadata` with generated metadata; explicit keys win.
- Never place API keys, prompts, or raw LLM output in metadata.
- Add tests for each environment detector.

### P0.6 Structured Errors and Exit Codes

- Keep semantic exit codes, but make docs, tests, and implementation match exactly.
- Parse Keito error body fields: `error`, `error_description`, `message`.
- Preserve JSON error shape:
  - `error: true`
  - `code`
  - `message`
  - `suggestion`
  - optional `details`
- Respect `Retry-After` for 429.
- Retry network/5xx with exponential backoff and jitter.
- Send all tracing/logging to stderr.
- Do not print secrets.

### P0.7 Output Contract

- Introduce a typed `OutputFormat`/`OutputMode` with `auto`, `json`, `table`.
- Keep `--json` for compatibility; consider `--output json|table|yaml|csv` for future.
- Normalize JSON command output so agents do not need to understand raw Keito API envelopes.
- Keep raw fields available where useful, but stable top-level command responses should include:
  - `id`/`entry_id`
  - `project`
  - `project_id`
  - `task`
  - `task_id`
  - `spent_date`
  - `hours`
  - `duration`
  - `source`
  - `metadata`
- Use table rendering for TTY only.

### P0.8 Documentation and Man Pages

- Update README examples against the chosen API contract.
- Update `docs/agent-guide.md`.
- Update `AGENTS.md`.
- Next phase: generate man pages recursively:
  - `keito.1`
  - `keito-auth.1`
  - `keito-auth-login.1`
  - `keito-time-start.1`
  - etc.
- Treat generated man pages as the authoritative agent command reference.
- Add checks that every referenced man page exists and that pages do not contain stale production API fields.
- Add `keito completions <shell>` before GA or document why it is deferred.
- Align public docs with the CLI or file platform docs tasks to fix them.

## P1 Task List: Missing Product Surface

### P1.1 Expense Commands

- Add `keito expense log`.
- Add `keito expense list`.
- Add expense category discovery command if the API exposes categories; otherwise add a platform API task.
- Support LLM usage metadata:
  - `--agent-id`
  - `--session-id`
  - `--model`
  - `--input-tokens`
  - `--output-tokens`
  - `--source`
- Add tests for `source=agent` expenses.

### P1.2 Time Entry CRUD

- Add `keito time edit <id>`.
- Add `keito time delete <id> --yes`.
- Support locked/approved conflict handling.
- Add destructive-action guard with non-interactive `--yes`.

### P1.3 Clients and Reports

- Add `keito clients list|show`.
- Add `keito reports summary`.
- Add `keito reports entries` only if backed by a stable API endpoint.
- Add `--from`, `--to`, `--group-by`, `--source`, `--agent-id`.

### P1.4 Config Commands

- Add `keito config show`.
- Add `keito config set <key> <value>`.
- Add `keito config init`.
- Add `keito config list` alias if public docs keep that name.
- Support config keys:
  - `api_url`
  - `account_id`
  - `default_output`
  - `timezone`
  - `agent.default_agent_id`
  - `agent.default_source`
  - `rounding.default`

### P1.5 Shell Completions

- Add `clap_complete`.
- Add `keito completions bash|zsh|fish|powershell|elvish`.
- Add tests that command renders non-empty output for each supported shell.

### P1.6 Release and Packaging

- Add or verify `cargo-dist` config.
- Add release artifacts for macOS, Linux, Windows.
- Add Homebrew formula validation.
- Add `cargo publish --dry-run`.
- Add `cargo install --path .` smoke test.
- Ensure binary name is `keito`, package name remains clear.

## P2 Task List: Hardening and Agent UX

### P2.1 Offline Queue and Sync

- Add local queue storage under platform data dir.
- Queue duration-based entries and expenses when network/server unavailable.
- Add `keito sync`.
- Add conflict/idempotency strategy.
- Add queue inspection and discard commands.

### P2.2 Workspace and Project Resolution

- Add optional local project mapping config.
- Add git remote/project code matching only if deterministic.
- Keep exact name/code/ID matching; avoid fuzzy automatic selection.
- Provide suggestions in JSON errors, not prompts.

### P2.3 Observability

- Add `--verbose` structured logs to stderr.
- Add request IDs in debug logs.
- Add latency logs without secrets.
- Add API error body redaction tests.

### P2.4 Agent Skill Package

- Create/publish an agent skill package once CLI commands are stable.
- Keep the skill small and have it rely on `keito --help`.
- Include recovery workflows for auth errors, project/task not found, timer conflicts, and network queue.

## Platform API Tasks

These are in the production app repo, not this CLI repo:

- Update public docs to match the shipped v2 contract, or clearly label `/v1` as future/unreleased.
- Fix `docs/openapi-v2.yaml` production server from `app.keito.io` to `app.keito.ai` if v2 remains canonical.
- Add or document timer stop endpoint.
- Add expense category list endpoint if `expense log --category <name>` is required.
- Add project/task show endpoints if CLI should avoid client-side list-and-find.
- Confirm whether task assignment should be project-scoped or workspace-global.
- Confirm metadata indexing claim in public docs; current code stores JSONB but does not obviously expose metadata filters.
- Add staging credentials and disposable workspace for CLI smoke tests.

## Test Plan

### Unit Tests

- Duration parsing and validation.
- Date parsing.
- Source enum parsing.
- Metadata merge and 4KB validation.
- Agent environment detection.
- Config precedence.
- Error mapping and suggestions.

### Mock API Tests

- Auth `users/me`.
- Project/task/time/expense list envelopes.
- Timer lifecycle.
- Expense logging.
- Locked/approved conflict.
- Rate limit with `Retry-After`.
- Server retry.

### CLI Integration Tests

- `--help` for all commands.
- JSON output parseability for all commands.
- Non-TTY auto JSON.
- Quiet mode.
- Completions output.
- Man-page generation output count.

### Staging Smoke Tests

Requires disposable credentials:

1. `keito auth status --json`
2. `keito auth whoami --json`
3. `keito projects list --json`
4. `keito projects tasks --json`
5. `keito time log --project <id> --task <id> --hours 0.01 --notes "cli smoke" --json`
6. `keito time start --project <id> --task <id> --notes "cli smoke timer" --json`
7. `keito time running --json`
8. `keito time stop --notes "cli smoke done" --json`
9. `keito expense log ... --source agent --json` once expense category support exists.

## Suggested Work Order

1. Implement D1 as v2 and decide D2.
2. Update fixtures and tests to the v2 contract.
3. Make existing auth/projects/tasks/time commands pass against production-shaped mocks.
4. Add agent metadata/source support.
5. Add expense commands.
6. Add edit/delete/config/completions/man-page recursion.
7. Add staging smoke tests.
8. Update docs and release packaging.

## Definition of Production Ready

- Existing core commands work against production-shaped mocks and staging.
- Public docs, README, man pages, and code agree on one API contract.
- Agent entries are created with `source=agent` and valid metadata.
- Human CLI entries are created with `source=cli`.
- Timer start/running/stop/discard lifecycle is reliable.
- JSON output is stable and parseable.
- Exit codes are documented and tested.
- No secrets are printed in normal, error, or verbose modes.
- Release artifacts install and run on macOS, Linux, and Windows.
