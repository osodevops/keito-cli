# Developer QA ownership repair

The `dev-tools` Rust protocol and matching Python QA protocol carry only bounded
cancellation and process-ledger metadata. They grant no capture, profile, usage,
authentication or runtime-pair authority. Neither module is installed in the
21-file Rust/thin-shell skill payload. The developer API now reexports the single
Unix implementation in `process_ownership`; that shared implementation compiles
in default library builds. Product call sites and the guard still need wiring.
Compilation alone is not default-artifact ownership or runtime acceptance.

The protocol requires a valid terminal followed by EOF. A nonce, identity, schema,
state, I/O, forwarding or budget fault permanently fails that endpoint and its
owner; later valid frames cannot restore clean settlement. Cancellation closes
admission separately from resource cleanup. REGISTER/WAITED facts alone never
authorize signalling another process or prove a reserved group is absent.

Metadata limits apply across an operation's shared send/receive/forwarding budget:
256 bytes per frame including newline, 1024 pending bytes, 4096 registrations,
16384 frames and 4 MiB total metadata. Each poll makes at most eight read attempts
and parses at most 32 frames. Reason codes contain no offending input.

Containing-owner failure also prevents clean terminal emission from every other
admitted endpoint. Root forwarding uses the same closed metadata ledger even
without an upstream socket; unfinished registrations prevent successful finish.
Each receive, local forward and actual send counts toward its operation budget.

The manager interface currently returns an untimed boolean. A first proof polled
at or after the shared cleanup deadline is rejected, including a `True` returned
late. A previously accepted manager proof is cached and is not polled again or
given a fresh deadline. A manager must close its own admission permanently before
reporting success. These rules are tested with finite objects, not process actors.

This is a component repair. It is not yet connected to the process-owning runners,
native fixtures, guardian or core. The Python `SourceOwner` requires explicit
manager cleanup APIs; the existing `OwnedChildren` does not yet implement them.
Do not attach it to the old runner or infer cleanup from `manager.active == None`.

The Python owner now latches ordinary operation exceptions before cleanup and
terminal emission. Manager begin/poll/schema errors retain pending collection
within the same end; collected facts after a callback fault are cached separately
from clean proof. Failed close/finalization cannot replace the original operation
exception. Seven retained regressions plus the existing protocol/socket tests
pass (45 total). R35 independently approved this finalizer prerequisite with
53 passing methods, including eight additional reviewer cases. Those eight
cases are now checked in and called by Linux and Mac CI. This does not implement
the real multi-slot process manager or establish nested LIVE receipt acceptance;
R34-06 remains partial and open.

The shared Rust lease now has two closed cleanup policies. The legacy adapter
keeps its 1.8-second reserve; `HookShort` reserves 50ms, with 15ms TERM grace and
a 20ms handshake cap, inside the inherited hard end. Its optional keeper tag is
exactly `hook-short-v1`, never a numeric timeout supplied by a caller. Failed
parent control also caps local observation without clearing the first fault or
claiming a wire acknowledgement. At source `1ed602e`, 84 shared Rust state/socket
tests, default all-target checking, all-feature Clippy and formatting pass.
R37 independently approved extraction, policy plumbing and test promotion as
source prerequisites, with no required findings. Real product integration remains open.
The existing batch registry still uses the legacy policy. None of these checks
establishes an installed guard/core path, short-policy process acceptance or
default latency.

Ordinary verified command owners now select the closed `CommandProbe` policy.
READY and PARK share the remaining original owner operation end (hard end minus
50ms); they receive no new relative interval. The command-owner local bound
remains one second, cleanup remains 50ms and TERM grace remains 15ms. Capture
hooks retain `HookShort` and its 20ms setup cap. The public QA-owned image
constructor refuses `CommandProbe`; only the crate-private verified-image path
selects its fixed `command-probe-v1` keeper tag. This does not extend a hook cap,
change latency acceptance or prove actual cold startup and cleanup.

The one-second statement describes the constructor/owner lifetime. Installation
and explicit usage approval have earlier pair verification outside that owner
when unattached, so it is not a whole-command guarantee. The developer opaque
gateway tightens its supplied Scope before its own pair/image verification;
subsequent calls retain that original minimum. Stronger whole-command ownership
would require acquiring and reusing one owner before those earlier caller steps.

The shared Unix `Scope` now retains individual command slots and can settle one
without closing the containing owner. It consumes the original prepared Command,
returns pending keeper ownership, and progresses READY/PARK while bounded pipe
reads and writes continue. Global cancellation broadcasts every retained slot's
intent before polling waits; a failed slot or operation cannot restore clean
completion. Direct waits, keeper terminal EOF, group absence and output read-zero
EOF remain separate facts. Completed slots and their identities are not recycled.

Transient input and each output are capped at 1MiB; admitted buffer capacities
total at most 4MiB per Scope. Each pipe turn performs at most one 4096-byte I/O
attempt, and a Scope turn visits at most eight slots in rotation. WouldBlock,
interruption, a closed local descriptor or child exit never substitutes for
observed output EOF. Overflow retains the first fault while bounded draining
continues. Raw output has no Debug/Serialize implementation and stays outside
metadata reports. Result retrieval is single-use and requires timely resource
proof. Full retained reports are explicit snapshots outside the hot poll path.

At source `ff34067`, 98 shared state/socket/fake-stream tests, default all-target
checking, all-feature Clippy and formatting pass. The retained migration baseline
has 93 passes and one behavioral failure for containing admission being closed;
an earlier test-only type error is recorded separately. R38 independently found
two required corrections: retained input capacity was undercharged, and the last
finish-clock error did not propagate a zero cleanup bound. Their retained
regressions produced 99 passes and three behavioral failures. Source `a6b89ac`
passes all 102 focused tests, default all-target checking, all-feature Clippy and
formatting after correcting both paths; independent repair review is pending.
Input now charges its reserved Vec capacity, including unused space, and rejects
oversized reservations before spawn. Output allocation must fit its charged
capacity too. A final clock error broadcasts irreversible zero bounds before
marking finished; later clock recovery cannot reopen that observation interval.
The actual finish envelope is shared with clock-injected regressions, including
pending resources, accepted historical slots, earlier faults and an empty owner.
The only current Scope image entrypoint requires a QA-owned
0700 image. Sealed official images, cooperative core edges, signal/bootstrap
integration and actual product/probe/Python-manager call sites remain pending.
These tests do not execute the real command/guard chain or qualify its timing.

The remaining integration must preserve the guardian's actual `setsid`, imageFD,
ACK and timing boundaries; give controlled source processes cooperative owners;
and reserve same-session groups around opaque child commands. Group absence and
actual direct waits are separate requirements. Full ownership acceptance, live
nested receipt faults, default release latency, actual harness usage and billing
qualification remain open. Socket-only unit tests establish none of those gates.
