# keito CLI — Agent Integration Guide

This guide is for AI agents (Claude Code, Codex, OpenClaw, custom agents) that need to track billable time via the Keito platform.

## Environment Setup

Set two environment variables — no interactive login required:

```sh
export KEITO_API_KEY=kto_your_api_key_here
export KEITO_ACCOUNT_ID=co_your_company_id
```

Use a CLI-compatible full-access Personal Access Token. A Personal read-only
sync key cannot list project tasks or mutate time entries and therefore fails
the CLI preflight with exit code `1`.

Verify credentials:

```sh
keito auth status --json
```

```json
{
  "authenticated": true,
  "api_key_source": "environment variable",
  "account_id": "co_abc123",
  "workspace_id": "co_abc123",
  "api_key_valid": true,
  "credential_type": "full_access",
  "cli_compatible": true,
  "can_track_time": true
}
```

Exit code `0` = ready. Exit code `1` = fix credentials.

## Skill Installation

Install the packaged Keito skill into Claude Code and Codex:

```sh
keito skill install
```

Install one host only when needed:

```sh
keito skill install --agent claude-code
keito skill install --agent codex
```

For source-checkout installs, use the gstack-style setup entrypoint:

```sh
git clone --single-branch --depth 1 https://github.com/osodevops/keito-cli.git ~/.keito/keito-cli
cd ~/.keito/keito-cli
./setup
```

From each client repository, invoke the skill once:

```text
/track-time-keito
```

This writes `.keito/config.yml` with the selected Keito client, project, and
task IDs. Do not commit that file. For shared repositories, run
`keito skill team-init optional` or `keito skill team-init required` to add
agent guidance plus `.keito/config.example.yml`.

Check readiness:

```sh
keito skill doctor
keito skill status --json
```

## Discovery

### List Projects

```sh
keito projects list --json
```

```json
[
  {
    "id": "prj_abc",
    "name": "Acme Website",
    "code": "ACME",
    "client": "Acme Corp",
    "billable": true
  }
]
```

### List Tasks

Task availability is project-aware. Always pass the selected project so the
CLI returns its assigned tasks and production can enforce project/task and
member restrictions consistently.

```sh
keito projects tasks "Acme Website" --json
```

```json
[
  { "id": "tsk_001", "name": "Development" },
  { "id": "tsk_002", "name": "Design" },
  { "id": "tsk_003", "name": "Meeting" }
]
```

## Core Workflow: Start → Work → Stop

### 1. Check for Running Timer

Always check first — only one timer can be active at a time:

```sh
keito time running --json
```

The command exits `0` in both states. Inspect `running`: `false` means it is
safe to start; a returned entry with `running: true` means a timer is active.

### 2. Start Timer

```sh
keito time start --project "Acme Website" --task dev --json
```

```json
{
  "status": "started",
  "entry_id": "te_abc123",
  "project": "Acme Website",
  "task": "Development",
  "started_at": "2025-01-15T09:00:00Z",
  "source": "cli"
}
```

You can use project names, codes, or IDs. Resolution is case-insensitive.

### 3. Stop Timer

```sh
keito time stop --json
```

```json
{
  "status": "stopped",
  "entry_id": "te_abc123",
  "project": "Acme Website",
  "task": "Development",
  "duration_hours": 1.5,
  "duration_seconds": 5400,
  "source": "cli"
}
```

### Discard a Timer

If a timer was started by mistake:

```sh
keito time stop --discard --json
```

The time entry is deleted and no duration is recorded.

## Batch Logging (No Timer)

Log time after the fact with an explicit duration:

```sh
keito time log --project acme --task dev --duration 1:30 \
  --date 2025-01-15 --notes "Fixed auth bug" --json
```

Duration formats: `1.5` (decimal hours) or `1:30` (HH:MM).

## Error Recovery

Every error returns a structured JSON response with recovery hints:

```json
{
  "error": true,
  "code": 3,
  "message": "Conflict: A timer is already running.",
  "suggestion": "keito time stop"
}
```

### Exit Code → Action Table

| Exit Code | Meaning | Recovery Action |
|---|---|---|
| 0 | Success | — |
| 1 | Auth/permission error | Check the key is valid, full-access, and permitted; read-only sync keys cannot track time |
| 2 | Invalid input | Fix arguments (bad duration, missing flags) |
| 3 | Conflict | Stop the existing timer first: `keito time stop` |
| 4 | Not found | Check project/task names: `keito projects list --json` |
| 5 | Rate limited | Wait for `details.retry_after_seconds` when present, then retry |
| 6 | Server error | Retry (automatic 3× backoff is built in) |
| 7 | Network error | Check connectivity, retry |
| 8 | Config error | Run `keito auth login` or set env vars |

### Structured Error Fields

- `error` — always `true` for errors
- `code` — numeric exit code (1–8)
- `message` — human-readable error description
- `suggestion` — recommended next command to run (when applicable)
- `details` — additional context, e.g. `{"available": ["Alpha", "Beta"]}` for not-found errors

## Integration Patterns

### Claude Code / CLAUDE.md

Add to your project's `CLAUDE.md`:

```markdown
## Time Tracking

Before starting work, run:
  keito time start --project "Project Name" --task dev --json

When finishing work, run:
  keito time stop --json
```

### Generic Agent Pattern

```
1. keito auth status --json          # verify credentials (exit 0 = ok)
2. keito projects tasks X --json      # choose a task valid for project X
3. keito time running --json          # inspect running=false before starting
4. keito time start --project X --task Y --json   # start timer
5. ... perform work ...
6. keito time stop --json             # stop timer
```

### Environment Variables Reference

| Variable | Required | Description |
|---|---|---|
| `KEITO_API_KEY` | Yes | API key (`kto_...`) |
| `KEITO_ACCOUNT_ID` | Yes | Company/account ID sent as `Keito-Account-Id` |
| `KEITO_WORKSPACE_ID` | Legacy | Alias for `KEITO_ACCOUNT_ID` |

## JSON Output

All commands support `--json`. When stdout is piped, JSON output is enabled automatically — no flag needed.

```sh
# Explicit JSON
keito projects list --json

# Automatic JSON (piped)
keito projects list | jq '.[0].id'
```
