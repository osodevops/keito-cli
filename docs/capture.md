# Private coding-agent capture

Capture records observed agent turn duration for review under your existing Keito identity.
It does not measure human effort, CPU time, verified delivery or permission to charge a client.
Numeric usage is disabled by default and requires separate device-local consent.
Qualified request observations remain partial; provider expenditure is unknown,
and missing counters are never reported as zero.

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
skill by default. Configuration merging uses the native Rust installer and the verified
Keito/core guard release pair; Bash and Git support the adapter scripts. It preserves
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
cannot resurrect the clock or count time while capture was disabled. Observing a disabled
or changed repository hint or remote also persists revocation; restoring its previous digest
requires explicit setup again. A refused Stop prevents later duplicate or pause/resume
commands from supplying a later completion time.

Prompt opens a supported turn and Stop checkpoints it. Session starts/resumes do not erase
previous completed intervals. Native Claude `prompt_id` and Codex `turn_id` identify turns;
missing identity is refused instead of falling back to another directory session. Interrupts,
missing boundaries and detected clock/reboot discontinuity produce incomplete evidence rather
than fabricated elapsed time. A short first turn remains exact; inter-turn idle is excluded.

Local storage is a private SQLite transaction ledger with full synchronization, a 100 MiB
ceiling and no silent eviction of unsent evidence. Frozen request bodies are limited to
16 KiB and 100 segments. Worker maintenance expires acknowledged request payloads after
seven days, including idle and consent-held queues;
minimized session/consent state remains for replay integrity until the bounded store is
explicitly managed. A full or damaged queue pauses capture with a recovery error. If SQLite refuses a
boundary (for example lock contention), separate bounded, private failure markers
record only hashed identities, timestamps and safe reasons. Publication never waits on
the authority lock; each marker has a unique immutable name and is synchronized before
acknowledgement. Clock transitions acquire SQLite before a bounded 25 ms authority lock,
then recheck refusal state; no network runs under that lock. The missed boundary is explicitly refused,
not described as saved. Status exposes storage_paused and the failure reason.
After fixing storage, explicitly run `keito capture recover --confirm`; it marks unfinished
intervals incomplete and never reconstructs missing seconds. Because markers contain no
customer or session identity, potentially affected unacknowledged complete revisions are
conservatively held across the local queue with `REFUSED_BOUNDARY_REVIEW_REQUIRED`.
Their frozen bytes remain unchanged, and recovery or reapproval cannot release them.
Recovery clears only the exact marker snapshot it reconciled; a concurrent new refusal
remains paused and requires another explicit recovery.

Installed hooks publish a private, content-free invocation flag before starting the CLI.
The guard alone holds its file lock and removes the exact flag only after a fixed-byte
acknowledgement from a durable core result. Startup stalls, watchdog termination, missing
acknowledgements and abandoned preparations therefore remain visible to the next core
operation. Any pending invocation blocks delivery; an abandoned one pauses collection and
holds potentially affected frozen complete evidence for review. Explicit recovery cannot
clear a live invocation. The flag contains no native session identity, hook JSON or paths.
This protects failures after the guard first creates durable local state; it cannot observe
a guard that never reaches its first filesystem instruction.

Capture state defaults to the effective OS user's configuration directory, independently
of hook `HOME`, `XDG_CONFIG_HOME` and repository input. An explicit `KEITO_CAPTURE_HOME`
is an operator or supervisor setting and must name the same private ledger for both guard
and core. Invocation directories require owned, private permissions and paths without
symlink components.

## Separate numeric usage consent

Metadata-only timing consent never authorizes transcript reading. From the approved
repository, inspect or disable one namespace independently:

```sh
keito capture usage status --repo --agent codex --json
keito capture usage disable --repo --agent codex --json
```

Explicit enable requires the current verified release pair, managed hook registration,
a canonical owned HOME, and the actual native harness image. The supported parser
versions are Claude Code 2.1.319 and Codex CLI 0.167.0. Supply owned canonical paths:

```sh
keito capture usage enable --repo --agent codex \
  --profile-home /absolute/approved/home \
  --harness-executable /absolute/codex-launcher \
  --native-executable /absolute/codex-native --confirm
```

Use `--agent claude-code` for Claude. A native launcher may omit
`--native-executable`; a JavaScript launcher must identify its native image separately.
Enable performs bounded image qualification, one exact-native `--version` probe and
full managed-install inspection. Functional paths stay in a private device profile
record. SQLite consent and billing evidence contain no source paths. Status and
disable do not inspect harness profiles or transcripts; disable remains available
when a profile or release pair needs repair.

A normal guarded hook checks persisted consent inside its SQLite transaction before
loading a profile, inspecting origin metadata or opening a source file. It requires
an exact supported version, unchanged approved installation/profile/runtime, a live
guard invocation and a coherent same-user native-image ancestor within at most eight
hops. It inspects PID, start time, parent and image metadata, without reading process
arguments, environment or enumerating processes. PATH, hook JSON, HOME and a version
argument alone cannot grant this authority. These cooperative checks do not establish
provider attestation or protection against a malicious process running as the same OS user.
Darwin retains the documented kernel path-to-image race.

The next qualified Prompt establishes an identifier-only historical exclusion baseline
and opaque source identity. Provisional historical request identifiers remain excluded
when later finalized. Stop/SessionEnd cannot repair a missing baseline, and a replaced
source requires another qualified Prompt. Symlinks, FIFOs, hardlinks, directories,
wrong sessions/paths/versions and changed sources remain unavailable. Pauses,
revocation, reapproval, refused boundaries and native gaps require a fresh baseline.
Profile, registration, runtime or native image changes require explicit reapproval.
A late lease failure rolls back new numeric finalization while preserving legitimate
timing and previous frozen evidence. Numeric disable affects only that namespace;
metadata timing consent remains separate.

Only minimized request identifiers, optional model/provenance and nullable counters
can enter usage evidence. Observations remain `PARTIAL`; they do not establish full
session coverage, streaming finality, transcript lag or provider expenditure. The
owned inert source-fixture tests exercise the normal command route; real pinned-harness
trust/lifecycle qualification, final release timing and demonstrations remain separate
required records. No usage observation automatically creates a charge or expense.

Canonical request data contains harness/session identifiers, versioned mapping,
absolute lifecycle instants, exact integer seconds, completeness and either qualified
partial usage or explicit unavailability. It contains no prompts, assistant prose,
transcript, source code, tool arguments, raw branches, full repository paths or API keys.
Metadata-only capture does not read transcript files. Known credential/path patterns
are refused in identifiers; this is not a claim to detect every possible secret.

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
