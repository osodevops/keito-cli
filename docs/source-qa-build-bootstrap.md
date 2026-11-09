# Developer compiler bootstrap draft

This Python standard-library owner is developer QA only. It is not installed or
called by Keito's Rust CLI, guard, hooks or adapters. The source-reviewed Rust
keeper cannot own its own first compiler before being built. This draft supplies
that separate bootstrap path; no process actor or compiler has run through it.

Run only pinned system Python with `-I -S -B` under the proven inherited kernel
boundary and an explicit private spec. The spec binds canonical Cargo and owner
source hashes, exact argv/cwd, private HOME/harness/state/TMP/target roots, a
closed offline environment and one finite lifetime. Missing environment/policy
markers refuse before actors. The marker is a refusal guard, not proof of kernel
policy. Actual direct/alias denial checks and source/copy hashes belong to the
coordinator's future build receipt, never normal profile contents.

A forked sibling keeper reserves its own live PID as PGID. Parent alone owns the
returned keeper PID and Cargo Popen handle before receipts. Catchable cancellation
is blocked across spawn/handle assignment; the compiler child restores the
original signal mask before exec. Keeper ignores owner-group INT/TERM and reacts
to its control lifeline. No parent cached-group query or signal exists. Command
ancestry is unchanged by this sibling; opaque tools that escape groups/sessions
are outside a group-absence proof.

PARK is intent until the bounded control drain. Complete rejected lines poison
proof but permit bounded failure-only draining; partial/budget yield permits no
group work. Fatal unknown transport stops group authority, leaving FAILED/unknown.
Parking-loss fallback is a one-time action against the keeper's own still-reserved
member group, including itself, with no GONE/proof. Expiry reserves final1.8s
INSIDE the granted lifetime. TERM grace starts once. This single-command bootstrap
supports ONE immutable CLEAN deadline; later requests are refused, not silently
accepted only in the parent. It is not the Rust component's decreasing-bound API
or a complete Scope implementation.

Normal Cargo leader completion is not group absence. After CLEAN, source-bound
GONE, actual direct waits, RELEASE/write EOF, successful keeper exit and empty
terminal EOF are required before the proof deadline. A nonzero compiler exit is
still a failed command even if its resource cleanup settles. Failure never
rehabilitates proof. Available direct waits continue to be collected after a
failed proof deadline within the ORIGINAL lifetime, without new signals or a new
proof budget. FIRST is exclusive and precedes later wait-only facts; FINAL and a
bounded fact-only stdout fallback preserve failures/unknowns if receipts fail.
Unknown handles, lost helper authority, late observations and missing receipts
must block acceptance; a dead owner's direct waits cannot be reconstructed.

The current checks are pure fake transport/kernel/wait/policy tests only. No
fork, Popen, signal/group actor, Cargo invocation or real harness has used this
owner. Independent source review and bounded bootstrap OS acceptance remain
required. The proposed Rust actor driver also remains uncompiled text. Parent
GroupLease runtime acceptance, Scope/core/native/Bash/Python integrations, LIVE
receipt faults and full BOTH-harness numeric P0-B/default artifact timing/images
and workers/financial/browser/recovery/rehearsals/release remain open.
