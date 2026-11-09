# Reserved group implementation draft

This developer component is not connected to product hooks, the guard, the core,
native fixtures or Python tools. Default builds exclude it. The keeper example
is never installed. Its state/socket checks are not process acceptance.

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
join callback cannot return to a caller for reuse. Scope registry and receipt
forwarding are still missing.

The granted lifetime reserves its final 1.8 seconds for failure cleanup. Expiry
starts cleanup at that earlier trigger, without extending the immutable hard end.
Cleanup advances incrementally, receiving deadline reductions before each step.
TERM grace starts once. Every group syscall checks the current absolute end.
The same decreasing end covers GONE, RELEASE and empty EOF. RELEASE plus write EOF
seals the last shared bound; a later smaller request is explicitly refused with
GROUP_BOUND_SEALED, rather than silently shortening only the parent. That terminal
seal is not permission to restart a deadline or perform another group operation.

If the original owner group disappeared before PARK, the keeper has a failure-only
last action against its own still-reserved group: SIGKILL includes the keeper.
This cannot emit GONE or provide a clean proof, and cannot give a dead parent
wait facts. No parent-side cached group operation or replacement parking actor is
introduced. This topology still needs independent review and finite OS acceptance.

`poll_cleanup` always attempts both outstanding direct waits, even on a prior
fault, clock failure or expired proof deadline. `report` retains the first safe
reason, up to eight distinct secondary reasons, actual returned statuses and
remaining unknowns. A later valid frame or wait cannot rehabilitate proof. Failure
closes still-owned stdin and the control writer; the keeper then makes its
bounded failure cleanup attempt. Transferred stdin/stdout/stderr must have a
caller-scoped cancellation/drain/EOF owner; keeper EOF is not output EOF.
Drop is only a bounded failure fallback. Explicit failed-handle registry retention
and reporting remain caller responsibilities and are not yet integrated.

The parent accepts exactly one GONE in Cleaning. Pre-READY cancellation still
requires the actual matching READY before GONE. Duplicate terminals, wrong-phase
frames, partial trailing bytes, missing EOF and failed waits cannot form proof.
Opaque tools that change groups/sessions or daemonize are outside group absence;
this does not prove an arbitrary subtree absent or reap non-child orphans.

The original R19 source at 263b918 remains immutable for later actor RED. Current
regressions use the actual settlement/session methods with fake clocks, wait
handles and syscalls plus finite socketpairs. No keeper/workload/group/signal actor
has run. Full Scope/core/native/Python integration, real nested LIVE receipt
failures, default release FIRST/p95, BOTH real full numeric harnesses, images and
workers, financial/browser tests, rehearsals and guarded release remain gates.
