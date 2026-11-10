# Developer ownership registry

`source_qa_scope::GroupRegistry` is the first integration layer above the reserved
group lease. It is compiled only for Unix developer tools and is not installed
with the CLI or capture payload. It currently manages opaque same-session group
edges. Cooperative core edges, pipe draining/EOF and upstream terminal reporting
still require integration.

The registry reserves bounded slot capacity before starting a keeper. It stores
the returned lease before invoking the registration publisher. An unsuccessful
publisher cannot discard that slot or reopen admission. Original I/O errors are
returned to the caller; safe sticky stage codes and bounded secondary codes stay
in reports. Slot IDs are bound to their registry and cannot target another one.

`spawn_group`, `await_ready` and consuming `start` preserve the existing lease
protocol. `begin_cleanup` closes admission and broadcasts the same minimum
absolute CLOCK_MONOTONIC end to every retained slot before any polling or waiting.
One failed begin does not skip a sibling. Polling attempts all slots, including
after a previous error. Both failed and successful slots remain in the registry
for explicit reports; a known direct wait never replaces a missing group proof.

`finish` uses that same end and a fresh completion clock. It never grants a new
per-slot cleanup allowance, never rehabilitates late observations, and records a
deadline or clock failure even when earlier group facts were already complete.
After finish, a request to reduce the sealed bound is refused in sticky report
state without another group operation. Drop is a failure fallback, not the normal
completion API. A failed lease also caps its local retention deadline to the
registry end so dropping several failed slots cannot multiply the allowance.

Reports contain each retained group's known direct-handle PID/status facts and
unknowns, separately from the aggregate flags. Those PIDs are metadata, not
permission to query or signal a cached group. Only the keeper retains that
authority. The registry never queries or signals a PID/group itself.

This is a group batch finalizer, not yet a complete command/output runner. It
does not provide output-pipe access or claim output EOF: `output_eof_proven` is
always false. Do not use it to attest an opaque guardian core across `setsid()`;
that core must remain a cooperative owner with leases inside its own session.
There is no capture/profile/auth approval constructor or upstream SETTLED API.

The first normal-only integration baseline reproduced six behavioral failures;
the repaired source passed 90 selected state/socket tests, including the original
eight registry cases and seven additional controls. The independent finite OS
comparison of the existing keeper interface remains scoped to that interface:
it bypasses parent GroupLease/registry APIs.

The separate R33 review at `257f5c0` accepted four finite public API cases on
macOS: two-group clean completion, actual registration-receipt collision,
invalid repeated start, and expired-bound failure retention. The last case
observed late natural expiry and does not prove timely cancellation. All actual
direct waits completed; QA lifeline EOF is not product output EOF. The original
R32 early failure remains unaccepted and its missing reason is not reconstructed.

The private actor and helper explicitly supply UID-derived zero encoding
metadata on macOS. The actor validates this exact value and its closed platform
environment; arbitrary extra keys remain refused. No operator environment value
or credential is copied. This developer QA correction is not installed capture
runtime qualification. See the [scoped evidence](pr-evidence/group-registry-r33-20261110.json).

Nested LIVE receipt faults, cooperative core,
native/chain/Bash/Python-manager integration, default FIRST/p95 and BOTH real full
numeric harnesses remain required. Product timing limits, profile isolation,
CLIProxyCLI and production behavior are unchanged. The current 1.8-second group
cleanup reserve still requires integration with the shorter product deadlines;
passing these finite cases does not authorize increasing those deadlines.
