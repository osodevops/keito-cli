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
pass (45 total); independent review is pending. This does not implement the real
multi-slot process manager or establish actual nested LIVE receipt acceptance.

The remaining integration must preserve the guardian's actual `setsid`, imageFD,
ACK and timing boundaries; give controlled source processes cooperative owners;
and reserve same-session groups around opaque child commands. Group absence and
actual direct waits are separate requirements. Full ownership acceptance, live
nested receipt faults, default release latency, actual harness usage and billing
qualification remain open. Socket-only unit tests establish none of those gates.
