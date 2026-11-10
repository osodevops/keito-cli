# Reserved group implementation draft

This developer component is not connected to product hooks, the guard, the core,
native fixtures or Python tools. Its state machines now live in the shared
`process_ownership` Unix library module, with developer compatibility reexports.
Default library builds compile that shared source; real product call sites and
guard roles are not wired yet. The keeper example is never installed. Its
state/socket checks are not process acceptance.

The lease is only for an opaque command in the current session. A guardian core
still calls `setsid()` and must instead be a cooperative source owner whose own
opaque children receive leases inside that session. The parent never queries or
signals a cached PGID. The live sibling keeper alone operates on its own
PID-reserved group. It stays a member until the original prepared command joins,
parks outside it, and reports GONE only after ESRCH. Absence irreversibly disables
all subsequent group queries/signals. Direct waits remain separate facts.

Call `spawn_keeper`, register the returned object locally, then call `await_ready`
and `start`. No READY read occurs before the caller receives the keeper handle.
`start` consumes the original prepared Command once, preserving its executable,
arguments, environment, descriptors and existing pre-exec checks; its appended
join callback cannot return to a caller for reuse. The developer
[group registry](source-qa-scope.md) now retains these objects before publishing
registration and coordinates one batch cleanup. Receipt forwarding, cooperative
core edges and output-pipe ownership remain unintegrated.

The granted lifetime reserves its final 1.8 seconds for failure cleanup. Expiry
starts cleanup at that earlier trigger, without extending the immutable hard end.
Cleanup advances incrementally, receiving deadline reductions before each step.
TERM grace starts once. Every group syscall checks the current absolute end.
A receive result distinguishes a frame, WouldBlock quiescence, EOF and a read
budget yield. A read-budget yield or the eight-frame turn cap skips group work;
already-buffered reductions are consumed on a later bounded turn first. Failure
cleanup continues to consume canonical CLEAN minimum bounds without clearing its
first fault, restarting grace or reopening normal control/proof. The actual
keeper_main turn function is shared with fake-transport regressions, so tests
exercise these dispatch decisions rather than separate model methods.

PARK parsing records only one pending intent. The membership change, its
failure-only self-group fallback and PARKED response occur only after the same
bounded drain has applied readable minimum bounds. Read/frame-budget yields and
partial controls retain intent without group work; a fatal transport stop clears
it without executing it. Duplicate pending PARK is a control failure. A timely
ordinary PARK retains its original response, and a failed PARKED write still
enters bounded failure cleanup after the membership change.

A rejected complete line has a known consumed boundary. It poisons proof but
continues the bounded failure-only drain, so a valid shorter CLEAN behind it is
applied before group work. WouldBlock with a partial frame yields instead of
claiming the queue empty. Fatal reads, wire overflow, an unterminated frame limit
or truncated EOF leave no trustworthy drain boundary: the keeper permanently
disables group operations and exits failed. Even a later valid frame cannot
restore that authority. Physical cleanup may then remain unknown; no GONE or
clean proof may be issued, and caller registry/wait reporting must retain that
failure. This is a conservative authority stop, not evidence of group absence.

The same decreasing end covers GONE, RELEASE and empty EOF. RELEASE plus write EOF
seals the last shared bound; a later smaller request is explicitly refused with
GROUP_BOUND_SEALED, rather than silently shortening only the parent. That terminal
seal is not permission to restart a deadline or perform another group operation.

If the original owner group disappeared before PARK, the keeper has a failure-only
last action against its own still-reserved group: SIGKILL includes the keeper.
This cannot emit GONE or provide a clean proof, and cannot give a dead parent
wait facts. No parent-side cached group operation or replacement parking actor is
introduced. Its finite keeper-interface topology now has independent OS
acceptance; that comparison deliberately bypasses the parent lease API.

`poll_cleanup` always attempts both outstanding direct waits, even on a prior
fault, clock failure or expired proof deadline. `report` retains the first safe
reason, up to eight distinct secondary reasons, actual returned statuses and
remaining unknowns. A later valid frame or wait cannot rehabilitate proof. Failure
closes still-owned stdin and the control writer; the keeper then makes its
bounded failure cleanup attempt. Transferred stdin/stdout/stderr must have a
caller-scoped cancellation/drain/EOF owner; keeper EOF is not output EOF.
Drop is only a bounded failure fallback. The group registry retains failed slots
and reports until its caller completes explicit finish and inspects the facts.
Direct lease users still own those retention/reporting responsibilities.

The parent accepts exactly one GONE in Cleaning. Pre-READY cancellation still
requires the actual matching READY before GONE. Duplicate terminals, wrong-phase
frames, partial trailing bytes, missing EOF and failed waits cannot form proof.
Opaque tools that change groups/sessions or daemonize are outside group absence;
this does not prove an arbitrary subtree absent or reap non-child orphans.

The original R19 source at 263b918 remains immutable for the recorded actor RED. Current
regressions use the actual settlement/session methods with fake clocks, wait
handles and syscalls plus finite socketpairs. A separate independent finite OS
comparison passed both CLEAN controls, reproduced original263's expiry failure
and verified the candidate's timely physical cancellation with FAILED protocol.
It does not establish parent GroupLease or registry API runtime acceptance, raw
ESRCH instrumentation, orphan reaping or default latency. Full Scope/core/native/Python integration, real nested LIVE receipt
failures, default release FIRST/p95, BOTH real full numeric harnesses, images and
workers, financial/browser tests, rehearsals and guarded release remain gates.
