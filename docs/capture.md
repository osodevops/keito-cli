# Private coding-agent capture

Capture records observed agent turn duration for review under your existing Keito identity.
It does not measure human effort, CPU time, verified delivery or permission to charge a client.
Tokens and provider costs are currently unavailable; absence is never reported as zero.

This implementation requires the canonical agent-session API and workspace eligibility.
A released CLI, installed hook or successful local test does not prove the server is enabled
or your harness has been qualified. The initial qualification targets are Claude Code
2.1.319 and Codex CLI 0.167.0 on macOS; real lifecycle/trust/delivery qualification must be
recorded for the exact installed artifacts before calling them supported.

## Install selected adapters

Install the reviewed CLI release, then select only the adapters you use:

```sh
keito skill install --agent claude-code --agent codex
keito skill doctor --live --json
```

Scripted installation requires explicit targets. Interactive installation offers a target
picker. Authentication never silently installs both adapters. The CLI uses its bundled
skill by default. Safe configuration merging requires Bash, Git and Python 3. It preserves
unrelated hooks, settings and plugins. If integration-owned legacy registrations require
migration, review them and rerun the selected installation with `--confirm-migration`.

Review new or changed Codex hooks through the harness's normal `/hooks` trust flow and
check that lifecycle hooks are enabled. Keito never writes trust decisions or bypasses
managed settings. Doctor distinguishes configured credentials from verified live
authentication and a matching current owner/workspace/API-origin binding. Historical
delivery from another binding is not current delivery proof. Unknown trust or delivery never counts as ready.

## Approve a repository

From an intended project repository, use `/track-time-keito` to select an accessible
existing client/project/task, preview the mapping and accept the metadata-only policy.
The wizard writes a local mapping hint before requesting device-local consent. It never
creates clients or projects as an installation side effect.

For explicit scripted consent:

```sh
keito capture setup --agent claude-code --agent codex \
  --project PROJECT_ID --task TASK_ID --confirm
```

The runtime verifies current authentication and the selected project/task before storing
approval outside the repository. A cloned `.keito/config.yml` cannot authorize collection. Legacy YAML and JSON hints
use the same minimized mapping identity; ambiguous duplicate keys, malformed documents
and over-budget depth/alias input are refused without quoting the source. The parser has
no filesystem include or environment expansion feature enabled.
Changed mapping/policy or Git remote identity holds evidence until renewed approval. Linked
worktrees use the Git common repository identity; nested repositories remain separate.
A repeated identical setup preserves the binding and its pause state. Adding another
selected harness preserves the existing canonical session revision stream. A real mapping
or remote change holds prior queued evidence for review; an already-started native session
will not silently move to the new binding or restart at revision one.

## Automatic delivery on macOS

Save credentials with `keito auth login` first. The worker never copies an environment API
key into service settings. From the approved repository, explicitly install its supervisor:

```sh
keito capture setup --agent claude-code --agent codex \
  --project PROJECT_ID --task TASK_ID --confirm --install-worker
keito capture worker-status --json
```

This opt-in creates only an integration-owned launch agent for this repository. Its
operational settings necessarily contain the local executable, working directory and queue
paths; these remain on your device and are separate from billing evidence. No key is stored
in the launch agent. Existing unrelated launch agents are preserved. Binary changes make
the service stale; its expected-runtime digest also refuses delivery after replacement.
Explicitly repeat installation with the reviewed updated runtime.

Launchd starts and restarts the worker. Settings installed, service loaded, process running and delivery
verified are separate states. Failed or unavailable launchd observations are unknown,
not proof of service absence; uninstall keeps settings until absence is positively verified. Real restart/kill/recovery checks remain part of artifact
qualification. To stop/remove only this repository's worker service:

```sh
keito capture worker-uninstall --repo
# If the repository moved or was removed, discover its opaque owned worker ID:
keito capture worker-status --all --json
keito capture worker-uninstall --worker-id WORKER_ID
```

For a separately supervised foreground worker, or supported development environments:

```sh
keito capture worker --interval-seconds 5
```

The worker performs network I/O; hooks do not. An explicit sync is also a recovery trigger:

```sh
keito capture sync --json
```

A detached hook child is not the sole delivery mechanism. On unsupported platforms use
manual CLI time commands until native capture and supervision are qualified.

## Inspect and control

```sh
keito capture status --json
keito capture preview --session ORIGINAL_HARNESS_SESSION_ID --json
keito capture pause --repo
keito capture resume --repo
keito capture disable --repo
keito capture disable-all
```

Pause excludes its interval and holds queued uploads. Resume preserves earned time and
requires an unchanged approved binding. Disable revokes local consent; re-enabling requires
explicit setup again. None of these commands rewrites submitted accounting records. Revoking global or
harness consent abandons any unmeasured active interval as incomplete; reapproval
cannot resurrect the clock or count time while capture was disabled. A refused Stop
also prevents a delayed duplicate from supplying a later completion time.

Prompt opens a supported turn and Stop checkpoints it. Session starts/resumes do not erase
previous completed intervals. Native Claude `prompt_id` and Codex `turn_id` identify turns;
missing identity is refused instead of falling back to another directory session. Interrupts,
missing boundaries and detected clock/reboot discontinuity produce incomplete evidence rather
than fabricated elapsed time. A short first turn remains exact; inter-turn idle is excluded.

Local storage is a private SQLite transaction ledger with full synchronization, a 100 MiB
ceiling and no silent eviction of unsent evidence. Frozen request bodies are limited to
16 KiB and 100 segments. Acknowledged request payloads have seven-day local retention;
minimized session/consent state remains for replay integrity until the bounded store is
explicitly managed. A full or damaged queue pauses capture with a recovery error. If SQLite refuses a
boundary (for example lock contention), a separate bounded, private failure marker
records its hashed identity and safe reason. The missed boundary is explicitly refused,
not described as saved. Status exposes storage_paused and the failure reason.
After fixing storage, explicitly run `keito capture recover --confirm`; it marks unfinished
intervals incomplete, never reconstructs missing seconds, then clears the refusal marker.

Canonical request data contains only harness/session identifiers, versioned mapping,
absolute lifecycle instants, exact integer seconds, completeness and explicit usage
unavailability. It contains no prompts, assistant prose, transcript, source code, tool
arguments, raw branches, full repository paths or API keys. Metadata-only capture does not
read transcript files. Known credential/path patterns are refused in identifiers; this is
not a claim to detect every possible secret.

## Recover without changing accounting evidence

The worker checks local consent and whether any delivery is due before making even an
authentication request. It then rechecks authenticated owner, workspace and API origin
before delivery. A credential switch cannot transfer queued work to another person.
The server rechecks access on every revision and replay. Disabled/unavailable API, expired
authentication, network failure, rate limits and temporary server errors retain the original
payload. Retry-After and bounded backoff apply. A revision conflict or rejected payload is
held for inspection; no fallback ordinary time entry is created.

Use status and exact-session preview to inspect evidence. Never edit the database to bypass
a hold, delete unsent evidence to conceal a conflict, or enable legacy automatic logging as
a retry workaround. Review the original mapping and canonical server state with a workspace
administrator. Existing ordinary `time session-record` remains available for its legacy
manual/custom-metadata use; the new adapters never use its list-then-create strategy.

## Review before charging

Accepted capture evidence is nonbillable pending scoped review. An authorized reviewer
confirms the exact revision, safe notes and explicit charging decision. Time, token usage,
estimated provider cost and client charges remain separate. Approved sources retain their
financial snapshot; later evidence needs a reviewable amendment. End demonstrations at an
unsent draft/preview. Capturing activity never sends an invoice or charges a customer.
