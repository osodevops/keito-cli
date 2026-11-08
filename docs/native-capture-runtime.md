# Native capture runtime contract

Capture uses the Rust `keito-capture-guard` companion and the Rust CLI. No Python interpreter or development tests are installed in the skill payload.

The release workflow finalizes the guard first, embeds its exact SHA-256 into `keito`, and packages both executables with a versioned manifest containing both file hashes. The ordinary archive checksums and Homebrew checksum authenticate the distribution. An unsigned adjacent manifest alone grants no release identity. Source builds support manual commands; they do not manufacture a capture release anchor. `keito skill runtime --json` performs read-only pair verification.

Managed commands pin absolute core and guard paths plus `KEITO_CAPTURE_CORE_SHA256`, the installer-verified core hash. The guardian verifies the core through no-follow descriptors before executing it. The main CLI checks its compiled guard anchor and installed manifest whenever an invocation nonce is present. Replacement of a core, guard, or manifest refuses capture and retains the invocation flag. Doctor never executes the registered runtime to decide whether it is configured.

## Publication and acknowledgement

The guard uses effective-user passwd HOME unless the operator explicitly supplies `KEITO_CAPTURE_HOME`; raw stdin, working directory and inherited HOME never choose capture storage. Capture directories are private, and descriptor traversal refuses symlinks and foreign-owned ancestors.

Each invocation creates a cryptographically random, content-free `.preparing` file, takes its guardian-only lock, writes the fixed pending bytes, and flushes the file and directory. Preparation occurs outside the bounded publication lock: holding that lock over the initial macOS filesystem flushes exceeded the existing 25 ms contention budget in a controlled test. Rename to `.pending` and the subsequent directory flush occur under the publication lock. The core starts only after both are durable. Zero-length and partial `.preparing` files are recognizable uncertainty markers. A lock timeout retains the durable preparation and launches no core. Invocation-cap exhaustion publishes a minimized unique global refusal without evicting existing evidence.

The guardian never reads, parses or spools hook stdin. It passes stdin unchanged and exposes a private ACK pipe. Cleanup requires exactly `KEITO_CAPTURE_DURABLE_V1\n`, pipe EOF, child exit zero, completion within the internal deadline, and an unchanged private marker: bytes, length, owner, mode, link count, type, device and inode. A same-inode content edit is retained. Timeout, missing execution, extra ACK bytes, nonzero exit and interrupted preparation retain uncertainty. Only the owned, unreaped child's process group can be signalled.

The internal deadline remains 600 ms, with outer adapter TERM/KILL at 650/675 ms and a one-second host timeout. The parallel marker visibility gate remains 500 ms; the actual release artifact p95 target remains below 100 ms. Development correctness results do not qualify cold release latency.

## Executable descriptor lifecycle

macOS refused native Mach-O execution through `/dev/fd/N` with a CLOEXEC descriptor (`EBADF`), an inherited read descriptor (`EACCES`), and an inherited O_EXEC descriptor (`EACCES`). These failed qualification attempts are preserved. Apple's XNU syscall catalog has no public fexecve/execveat entry, and its devfs descriptor support modifies execute bits according to descriptor flags. The explicit Darwin implementation executes the fixed managed pathname after checking its device, inode and type immediately before exec. Linux uses the verified descriptor path. Neither platform silently switches launch methods on failure.

The narrow descriptor contract permits exactly standard I/O, private ACK descriptor 3, and one read-only, verified executable-image descriptor across kernel exec. All other inherited descriptors are closed.

The main executable starts synchronously, before CLI parsing or constructing Tokio. It marks the image descriptor CLOEXEC, obtains its installed path through macOS `F_GETPATH` or Linux `/proc/self/fd`, checks both that path and `current_exe` device/inode against the descriptor, and closes it on both success and error. Only the verified original path is retained for subsequent release-pair checks. The heavy runtime inherits no image descriptor. Darwin pathname selection has a residual check-to-exec race against a malicious process running as the same OS user; it does not provide fd-only execution or immutable image sealing.

These checks detect accidental replacement and stale installations. They do not establish security against an adversary running as the same OS user: that user can rewrite the adapter, replace environment assignments, or modify an owned executable inode in the hash-to-exec interval. Darwin also has the explicitly described final pathname-check-to-exec interval. No immutable-file or same-user tamper-resistance claim is made.

Windows and other unqualified capture platforms retain manual CLI commands and report capture as unsupported.

## Development evidence continuity

The abandoned production Python guard/installer/mapping implementation and Python-pin Doctor tests were archived in the dated task evidence before removal. The old Python adapter wrapper is no longer an active cargo test or installed payload. Its native replacements cover installer/mapping ownership and recovery in Rust module tests; protocol, exact ACK, timeout, concurrent publication and capacity faults are covered by `tests/native_capture_guard.rs`; compiled-anchor and file identity qualification is covered by `tests/native_runtime_pair.rs`. Successful CLI installation must also be qualified using the actual paired release artifact, because source-build refusal tests cannot establish that behavior.

Primary operating-system evidence: [Apple XNU descriptor support](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/miscfs/devfs/devfs_fdesc_support.c), [Apple syscall catalog](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/syscalls.master), and the local macOS execve(2) manual. The task record retains fetched source hashes and the platform's exact failing probe results.

## Reproducible paired-artifact qualification

After the guard-first/anchored-core build, extract the normal release archive into a fresh directory under a private run-owned root. Before executing either extracted binary, run:

```sh
KEITO_TEST_REAL_PROFILE_HOME=/absolute/original/profile \
  scripts/qualify-capture-artifact.sh /private/run-root/extracted /private/run-root
```

The development-only Rust driver is excluded from default installation. Its first artifact execution is the actual frozen dispatch/guard/core Prompt path, with private HOME, capture state, harness roots, config roots and PATH. It records the first result before assertions, verifies the actual core's exact private ACK on valid input, no ACK on malformed input, and no accepted boundary for replaced core/guard/manifest fixtures. It then measures 100 ordinary Prompt samples, with Stop boundaries between samples. The profile is read only to compare the three global configuration digests before/after; no profile restoration is attempted. Failed evidence remains in the printed run directory. The normal Unix release workflow runs this profile against the packaged/extracted artifact before upload.

Guard flushes use POSIX `fsync`, with bounded EINTR retry, in the original file/directory order. Rust `File::sync_all` maps to Apple's stronger `F_FULLFSYNC`, which introduced drive-wide flush latency in the first native candidate. Neither the guard nor SQLite claims power-loss durability stronger than the existing fsync/SQLite contract. No SQLite durability policy was changed.

The hook entry point performs one synchronous poll of its deterministic persistence operation and fails closed if that operation unexpectedly needs asynchronous work. It does not construct Tokio's general command worker pool. Authentication, sync and worker commands retain the normal asynchronous runtime. No updater or network operation runs before hook persistence.

The tampered-guard fixture invokes the actual qualified core against the changed guard file to prove its entry refusal. It does not claim that an unexecutable, corrupted guardian can start and publish uncertainty. The checked profile also opens an image descriptor, replaces the core pathname with an identical-byte file of a different inode, then invokes the actual core: startup must refuse before accepting the boundary. On macOS, the already-built qualification driver and its descendants run under an OS sandbox denying network; other hosts record when that facility is unavailable.

The first b43a5ca extracted-artifact probe failed at 866.572 ms with a retained
pending flag and no accepted session. That immutable baseline is retained. Later
pair/bootstrap and exact-ACK diagnostics were unqualified observations. The
qualification fixture now uses private symlinks to the caller's same selected Git
and explicit OS bash/sleep, recording paths and hashes instead of adding fresh
shell wrappers or choosing a faster Git implementation. A fresh reviewed source
and artifact still require a new quiet-host first probe.

The missing/non-executable/symlinked-core regression exercises both the bundled
dispatcher and actual plugin command, with direct-guard controls. Dispatch delegates
core refusals to the available guard so publication precedes refusal. A guard that
cannot start remains a separate pre-instruction limitation; no corrupted-guardian
startup durability claim follows from the core-refusal tests.


## Locked dependency preparation and output compatibility

The macOS CI job fetches locked dependencies before applying the offline test
boundary. A cache hit is optional. Local callers likewise run `cargo fetch --locked`
before `tests/run-macos-isolated.sh`; product tests retain inherited service-exec
and network denial.

The cold-cache regression was reproduced as a setup-gate failure with an empty
private Cargo cache. Locked public dependency preparation then populated that
cache, and an offline checker compilation check succeeded. Both stages denied access to the
operator HOME and denied real service execution after owned native marker/alias
and allowed controls. This is dependency-preparation evidence, not capture
latency qualification.

The source now requires Tokio 1.53.3 or later and locks that fixed release, locks
anyhow 1.0.103, and uses tabled 0.22/derive 0.12 to remove the unmaintained
proc-macro-error2 build dependency. Seven retained synthetic human-table outputs
cover typed tables, multiline/truncated notes, rounded borders, headers, fallback
and empty results; the migration preserves their exact output strings. These
source changes require a new reviewed build and first-artifact qualification.
The earlier 1e234ee paired build remains unexecuted evidence, not a qualified
artifact for the updated dependency set.

## Hosted CI follow-up and platform verification

A live Permit retains its verified read-only, close-on-exec marker descriptor
through clones, transaction validation and acknowledgement. This pins the inode
allocation after unlink, preventing a recycled device/inode pair from authorizing
a different marker. Snapshot/dead-marker probes still close immediately, releasing
any probe flock; the retained live descriptor does not own the guardian's lock.
This does not extend the documented same-user tamper or marker unlink/CAS guarantee.

Emergency refusal publication alone now uses POSIX `fsync` for the marker and then
the renamed marker's parent directory, preserving write/flush/rename/flush order
and error propagation. Apple's stronger drive-wide `F_FULLFSYNC` from Rust
`File::sync_all` exceeded the existing contention budget. SQLite's 500 ms busy
policy and synchronous setting, the 700 ms core test assertion, and all guardian
and adapter deadlines remain unchanged. This is the existing POSIX durability
contract, not a power-loss or unconditional filesystem-latency guarantee. The
contention test records event, emergency publication and total elapsed time even
when the unchanged assertion fails.

Windows automatic capture explicitly refuses before credentials, network, Store
or profile work; ordinary manual CLI/API/output and clap discovery remain covered.
Unix-only imports and resource fields are partitioned rather than warning-suppressed.
The platform test inventory is deliberate:

- `capture_store`: 36 POSIX-authority cases on Unix; six pure identifier/Git/mapping
  parser cases remain cross-platform.
- `capture_usage_integration`: 21 native consent/store/reader cases on Unix.
- `capture_transport`: 11 native consent/queued-store cases on Unix.
- Parser tests moved from the removed integration target to the private
  `capture_usage::parser_tests` library child in `tests/unit/capture_usage_parser.rs`.
  All 29 names/assertions remain: 27 pure decoder cases are cross-platform and two
  POSIX path/descriptor safety cases are Unix-only. Unix fixtures still invoke
  the real bounded readers and additionally compare pure decoder results. Pure
  decoder identity carries no path, file authority, consent or normal enablement.
- Mixed entry/supervisor suites partition native operations individually, retaining
  help, selection, and applicable doctor checks. `capture_unsupported` adds a
  Windows public-command matrix for unsupported diagnostics, no API requests,
  no stdout/ACK/readiness and unchanged private state/profile bytes.

Use `--lib capture_usage::parser_tests` for focused parser selection inside the
protected test environment. The old `--test capture_usage_parser` target no longer
exists; the full all-features/all-targets suite discovers the library cases.
Actual Windows hosted execution remains necessary; local source partitioning is
not a passing Windows runtime result. These changes also require fresh independent
review and a new paired-artifact qualification. The retained 689 source's FIRST
and p95 observations qualify only that earlier source/artifact slice.
