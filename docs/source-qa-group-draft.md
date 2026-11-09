# Reserved group implementation draft

This local developer component is source authoring, not process acceptance. It is
not connected to product hooks, the guard, the core, native fixtures, or Python
tools. Default builds exclude it. The keeper example is never installed.

The intended lease is only for an opaque command in the current session. A
guardian core still calls `setsid()` and must instead be a cooperative source
owner whose own opaque children receive leases inside that new session. The
parent never queries/signals a cached PGID. The live sibling keeper alone uses
its own PID as the reservation for the original group. It stays in that group
until the original prepared command joins, parks outside it, and reports GONE
only after ESRCH. After GONE it performs no more group queries or signals.
The parent separately waits its actual workload and keeper handles and checks
the keeper's terminal EOF. Workload success is distinct from group cleanup.

Every caller must register the prepared lease object before `start()` or receipt
I/O. The returned child handle is stored before later fallible work. Cancellation
starts cleanup before waiting and only shortens an existing absolute deadline.
Scope registry, forwarding and live receipt-fault integration are still missing.
The current Drop is a failure-only bounded fallback; failure or unknown waits
must remain unsettled and cannot count as complete cleanup. No guarantee that
arbitrary daemonized descendants remain in this reserved group is made.

Current checks compile the source and exercise only finite state/socket code.
No group keeper, opaque workload, product command or signal actor has been run.
Before actor acceptance, independently inspect prepare/spawn/READY/join/PARK,
keeper EOF/expiry and identity failure, cleanup/error propagation, handle retention
and wait ordering, output pipe handling, terminal+EOF and post-GONE authority.
Derive finite adversarial actor cases from R13/R16; retain every first outcome.
This document gives no release or actual harness qualification.
