# Keito CLI Production Readiness Plan

Date: 2026-05-04

## Summary

`keito-cli` now has production-compatible coverage for the core auth, project/task discovery, and time-entry timer workflows against the current Keito `app.keito.ai` v2 API. The remaining work in this plan tracks PRD features beyond the core timer flow and release hardening.

Validated sources:

- CLI PRD: `docs/keito-cli-prd.md`
- CLI man pages: `man/keito.1`, `man/keito-auth.1`, `man/keito-projects.1`, `man/keito-time.1`
- Production app repo: `/Users/sionsmith/development/oso/com.github.osodevops/keito`
- Production API docs/code: `docs/openapi-v2.yaml`, `src/app/api/v2/*`, `src/lib/api-v2-transformers.ts`
- Live host: `https://app.keito.ai`

Local tests pass with `cargo test --all-targets`, but those tests currently validate the mock API contract, not the real production contract.

## Live API Validation

Unauthenticated production probes on 2026-05-04:

- `GET https://app.keito.ai/api/health` returns `200` with healthy database and Redis checks.
- `GET https://app.keito.ai/api/v2/users/me` returns v2 JSON `401` for missing auth.
- `GET https://app.keito.ai/api/v2/projects` returns v2 JSON `401` for missing auth.
- `GET https://app.keito.ai/api/v2/me` returns a Next.js `404`; this is not a valid v2 API endpoint.
- `https://app.keito.io` fails TLS from this environment and should not be the default production base URL.

## Current CLI Surface

Implemented commands:

- `keito auth login|logout|status|whoami`
- `keito projects list|show|tasks`
- `keito time start|stop|log|list|running`
- Global `--json`, `--workspace`, `--quiet`, `--verbose`

Generated man pages cover top-level command groups and individual subcommands such as `keito-time-start(1)`.

## Production Blockers

| Area | Current CLI | Production API | Required change |
|---|---|---|---|
| API base URL | Fixed to default to `https://app.keito.ai` | Live production is `https://app.keito.ai` | Keep docs/man pages aligned. |
| Auth identity endpoint | Fixed to `GET /api/v2/users/me` | `GET /api/v2/users/me` | Keep models, tests, help examples aligned. |
| Pagination envelopes | Fixed to keyed envelopes | Returns `projects`, `tasks`, `time_entries` | Keep production-shaped fixtures current. |
| Create time entry date | Fixed to send `spent_date` | Requires `spent_date` | Keep request tests current. |
| Billable field | Fixed to send `billable` on create | Expects `billable` | Add update/edit commands when needed. |
| Time entry response names | Fixed to read nested names and `spent_date` | Returns nested `project`, `task`, `spent_date` | Keep rendering tests current. |
| Stop timer | Fixed to call `PATCH /api/v2/time_entries/{id}/stop` | Endpoint deployed and live-smoke-tested on 2026-05-05 | Keep regression coverage for start/running/stop. |
| Source/metadata | Create sends `source=cli`; metadata field exists | API supports `source` and `metadata` | Add CLI flags, auto-detection, validation. |
| Tests | Production-shaped fixtures for auth/projects/tasks/time | Production uses Harvest-style contract | Extend fixtures for expenses and edit/delete. |

## PRD Gaps

Remaining P0 gaps before the broader PRD is complete:

- Agent tagging: `--source`, `--agent-id`, `--session-id`, `--metadata`.
- Agent auto-detection from `OPENCLAW_AGENT_ID`, `CLAUDE_SESSION_ID`, `CODEX_SESSION_ID`, and `CURSOR_AGENT`.
- Safe JSON error behavior for production 400/401/403/404/409/429/5xx responses.
- Real API contract tests using production-shaped fixtures.
- Updated docs and regenerated man pages.

P1 gaps:

- `keito time edit` and `keito time delete`.
- `keito clients list|show`.
- `keito reports summary|entries`.
- `keito config show|set|init`.
- `keito completions <shell>`.
- Offline queue and `keito sync`.
- Expense logging for LLM costs.
- Full subcommand man pages.

P2 gaps:

- Homebrew release validation.
- Cargo publish validation.
- Cross-platform release workflow and smoke tests.
- Agent skill package/ClawHub publishing.
- Optional MCP server and SDK alignment.

## Recommended Plan

### Phase 1 - Make Existing Commands Production-Compatible

1. Done: change default API base URL to `https://app.keito.ai`.
2. Done: update `get_me` to call `/api/v2/users/me`.
3. Done: replace `PaginatedResponse<T> { data }` with production envelopes for projects, tasks, and time entries.
4. Done: update create request models to use `spent_date`, `billable`, `source`, and `metadata`.
5. Done: update response models to read `spent_date`, nested `project`, nested `task`, `source`, and `metadata`.
6. Done: fix table/status outputs to consume production response fields.
7. Done: rewrite API mock tests to use production-shaped fixtures.

### Phase 2 - Resolve Timer Stop Contract

Implemented locally:

- Added `PATCH /api/v2/time_entries/{id}/stop` in the production app repo.
- Updated CLI `time stop` to use the stop endpoint.
- Added recursive man-page generation and tests for agent-facing command pages.

Live verified on 2026-05-05:

- `keito time start --project "Website Redesign" --task "Development"`
- `keito time running`
- duplicate `keito time start` conflict handling with exit code 3
- `keito time stop --notes "..."`
- `keito time stop --discard`
- no-running `keito time stop --json` handling with exit code 4
- `keito time log --duration 0:15`
- `keito time list --limit 5`

### Phase 3 - Add Agent-Native Metadata

1. Add flags to `time start` and `time log`: `--source`, `--agent-id`, `--session-id`, `--metadata`.
2. Auto-detect agent context from known environment variables.
3. Default human CLI entries to `source=cli`; default detected agent entries to `source=agent`.
4. Add metadata size validation before sending requests.
5. Add tests for source filtering and metadata pass-through.

### Phase 4 - Fill Core PRD Commands

1. Add `time edit` and `time delete`.
2. Add `clients list|show`.
3. Add `reports summary|entries`, starting with the production reports endpoints that already exist.
4. Add `config show|set|init`.
5. Add shell completions generation.
6. Generate subcommand man pages and update `README.md`.

### Phase 5 - Production Hardening

1. Add smoke tests that can run against a staging Keito API with disposable credentials.
2. Add contract fixtures derived from `docs/openapi-v2.yaml`.
3. Add release checks for macOS, Linux, and Windows.
4. Add Homebrew and Cargo publish dry-runs.
5. Document API key creation and workspace/company ID discovery.

## Go/No-Go Criteria

Production-ready means:

- `auth status`, `auth whoami`, `projects list`, `projects tasks`, `time log`, `time start`, `time running`, and `time stop` all pass against staging and production-shaped mocks.
- JSON output is parseable and stable for every command.
- Exit codes match `README.md`, `docs/agent-guide.md`, and man pages.
- Agent-created entries arrive in Keito with `source=agent` and expected metadata.
- Human CLI-created entries arrive with `source=cli`.
- Man pages and README match actual flags and behavior.
- No command prints secrets.
