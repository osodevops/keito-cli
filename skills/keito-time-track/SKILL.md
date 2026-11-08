---
name: keito-time-track
description: >
  Set up explicit repository consent and inspect metadata-only coding-agent
  activity through the Keito CLI. Use when the user asks to install capture,
  map a repository, check queued activity, pause or resume capture, disable
  capture, or review agent time. Activity needs authorized review before billing.
---

# Keito Time Track

The CLI owns deterministic capture, the local consent ledger, exact observed
turn intervals and durable delivery. The skill selects and configures it.
Never measure time or create time/expense records from a model response.

## Setup

Confirm which supported adapters the user wants: `claude-code`, `codex`, or both.
Scripted installation always names targets. If the user already selected them,
continue without asking again. For example, for both:

```bash
keito skill install --agent claude-code --agent codex
scripts/setup-wizard.sh --agent claude-code --agent codex --install-worker
```

The CLI must expose `keito capture --help`. If it does not, install a qualified
CLI release before proceeding. `scripts/install-cli.sh` installs the public CLI;
it does not establish capture support or harness qualification. Authentication
belongs to `keito auth login`; never request or print API keys.

The explicit `--install-worker` option requests a local macOS background
uploader; the picker previews it before confirmation. Without that option setup
only approves local queuing, so do not claim automatic delivery. The CLI owns
service setup and must report its actual running/verified state.

The setup picker uses existing accessible clients, projects and project tasks.
It previews metadata-only collection and requires explicit mapping consent on
this device. A repository-carried `.keito/config.yml` is only a hint; it cannot
approve capture, choose a billing rate or authorize historical import. Failed setup restores the previous hint. After an
interruption, run setup recovery and inspect `keito capture status` before
explicit reapproval; core consent may have committed even when the wrapper
failed. Recovery must not replay that operation automatically.

## Inspect and control

```bash
scripts/status.sh
keito capture preview --session <exact-source-session-id> --json
keito capture sync
scripts/pause-resume.sh pause
scripts/pause-resume.sh resume
scripts/disable.sh
```

Pause, resume and disable target the approved current repository, including
unsent work; they do not rewrite previously submitted accounting evidence.
Use CLI status/preview for actionable holds and errors. Never open transcripts
or raw hook inputs for diagnostics. Never manufacture a successful delivery from
configured hook files or treat unknown usage as zero.

For adapter removal, explicitly select only the unwanted target:

```bash
installers/uninstall.sh --agent codex
```

Removal preserves unrelated/ambiguous commands and refuses to delete modified
skill payloads. Immutable hidden adapter generations remain for safe rollback;
installation never replaces an executable referenced by an existing hook. It does not delete capture evidence or restore previously disabled
legacy plugins automatically.

## Plugin qualification

Claude plugin registrations do not snapshot the running executable version.
They use `KEITO_HARNESS_VERSION` when explicitly provided, otherwise `unknown`.
Unknown means unqualified: never report a plugin as ready or live-delivery
verified from installed files. The direct installer records the selected
observed version; both routes still need real trust/dispatch qualification.
These scripts do not run a version-discovery subprocess on every hook.

## Capture and privacy contract

- `SessionStart` preserves session identity; `UserPromptSubmit` opens a turn;
  `Stop` checkpoints it; interruption/failure/finalization retain incomplete
  evidence. A session may have many completed turns.
- Hooks synchronously invoke the local CLI only, with a one-second fail-open
  ceiling. CLI SQLite durability precedes acknowledgement; workers handle
  network I/O and frozen retries separately.
- Default capture never uploads prompts, assistant prose, transcripts, source
  code, full local paths, branch names or Git diffs. The raw harness JSON is
  transient stdin to the allowlisting CLI and is never logged by these scripts.
- Observed agent duration is not human labour, CPU time, vendor expenditure or
  permission to charge. New evidence remains nonbillable pending review.
- Numeric usage and cost claims require separately qualified versioned evidence.
  These adapters alone establish no automatic usage or actual-cost claim.
- Installation cannot grant host trust, mapping consent, rate authority or live
  delivery verification. Codex uses `features.hooks`; follow its normal trust
  review and never bypass it. Changed hook hashes require renewed host review.
- Legacy plugin/direct-hook duplicates need an explicit migration confirmation.
  Do not silently run old financial hooks alongside the new implementation.
