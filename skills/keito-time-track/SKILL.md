---
name: keito-time-track
description: >
  Track billable AI coding-session time to Keito through the Keito CLI. Use
  when the user asks to set up repository time tracking, install or configure
  the Keito skill, log agent work, check tracking status, pause or resume
  tracking, disable tracking, or review today's source=agent time entries.
---

# Keito Time Track

You are helping the user manage Keito time tracking for coding sessions in
this repository. Time entries are created automatically by installed lifecycle
hooks when a session ends. Do not create time entries directly from the skill
body.

## Commands

When the user asks to set up Keito tracking for the current repo, or invokes
this skill directly with `/track-time-keito` or `/keito-time-track:keito-time-track`, run:

```bash
if ! command -v keito >/dev/null 2>&1; then scripts/install-cli.sh; fi
keito skill install --skip-skills-add
scripts/setup-wizard.sh
```

If `keito auth status` reports unauthenticated, stop before setup and tell the
user to run `keito auth login`. Do not ask for or print API keys.

When the user asks for tracking status, run:

```bash
scripts/status.sh
```

When the user asks to pause or resume tracking for the current session, run:

```bash
scripts/pause-resume.sh pause
scripts/pause-resume.sh resume
```

When the user asks to disable tracking for this repository, run:

```bash
scripts/disable.sh
```

When the user asks what was logged today, run:

```bash
keito --json time list --today --source agent
```

Summarise the returned entries by duration, project, task, and notes.

## Rules

- Never read or expose Keito API keys. Authentication belongs to the Keito CLI.
- Never write `.keito/config.yml` by hand. Use `scripts/setup-wizard.sh`.
- Treat `.keito/config.yml` as repository-specific. Do not reuse a config from
  another client or project.
- Never create a time entry from the skill body. The session-end hook owns
  duration measurement and writes exactly one entry per session.
- If the Keito CLI is missing, run `scripts/install-cli.sh`.
- If the Keito CLI is unauthenticated, tell the user to run `keito auth login`
  or set `KEITO_API_KEY` and `KEITO_ACCOUNT_ID`.
