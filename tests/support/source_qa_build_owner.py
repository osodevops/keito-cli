"""Developer-only compiler bootstrap owner. Never installed in the Rust CLI.

One opaque command, one immutable CLEAN end (later requests are refused). The
forked sibling keeper alone addresses its own PID-reserved group. This is not
Scope, an arbitrary daemon/subtree proof, or the real-harness qualification.
"""
import hashlib
import json
import os
from pathlib import Path
import resource
import signal
import socket
import stat
import subprocess
import sys
import time

CAP = 1_800_000_000
TERM = 15_000_000
FRAME = 256
WIRE = 4096


class Refused(RuntimeError):
    pass


def integer(value):
    if not value or len(value) > 20 or not value.isascii() or not value.isdigit():
        raise Refused("INTEGER")
    n = int(value)
    if str(n) != value or n > 2**64 - 1:
        raise Refused("INTEGER")
    return n


class Channel:
    def __init__(self, stream, nonce):
        if len(nonce) != 32 or any(c not in "0123456789abcdef" for c in nonce):
            raise Refused("NONCE")
        self.stream, self.nonce = stream, nonce
        self.pending = bytearray()
        self.wire = 0
        self.eof = False
        stream.setblocking(False)

    def charge(self, count):
        if self.wire + count > WIRE:
            raise Refused("WIRE_LIMIT")
        self.wire += count

    def send(self, body):
        if not body.isascii() or "\n" in body or "\r" in body:
            raise Refused("FRAME")
        data = (self.nonce + "|" + body + "\n").encode("ascii")
        if self.eof or len(data) > FRAME:
            raise Refused("FRAME")
        self.charge(len(data))
        try:
            count = self.stream.send(data)
        except OSError:
            raise Refused("WRITE") from None
        if count != len(data):
            raise Refused("SHORT_WRITE")

    def receive(self):
        for _ in range(8):
            end = self.pending.find(b"\n")
            if end >= 0:
                line = bytes(self.pending[:end + 1])
                del self.pending[:end + 1]
                if len(line) > FRAME or not line.isascii():
                    return "rejected", "FRAME"
                fields = line[:-1].decode("ascii").split("|", 1)
                if len(fields) != 2 or fields[0] != self.nonce or "\r" in fields[1]:
                    return "rejected", "FRAME_IDENTITY"
                return "frame", fields[1]
            if len(self.pending) >= FRAME:
                raise Refused("FRAME_LIMIT")
            if self.eof:
                return "eof", None
            try:
                data = self.stream.recv(FRAME)
            except BlockingIOError:
                return ("yield" if self.pending else "quiescent"), None
            except InterruptedError:
                continue
            except OSError:
                raise Refused("READ") from None
            if not data:
                self.eof = True
                if self.pending:
                    raise Refused("TRUNCATED")
                return "eof", None
            self.charge(len(data))
            self.pending.extend(data)
        return "yield", None


class Keeper:
    """Same-runtime bootstrap only; real syscalls live in child-only Kernel."""
    def __init__(self, kernel, hard):
        self.kernel, self.hard, self.end = kernel, hard, hard
        self.operation_end = hard - CAP
        self.first = None
        self.cleaning = self.clean_seen = self.intent = self.parked = False
        self.gone = self.released = self.stopped = self.abandoned = False
        self.term_end = None
        self.killed = False

    def fail(self, reason, at):
        if self.first is None:
            self.first = reason
        if not self.cleaning and not self.gone:
            self.cleaning = True
            self.end = min(self.end, at + CAP)

    def check(self, clock):
        if self.stopped or self.abandoned:
            raise Refused(self.first or "AUTHORITY_STOPPED")
        at = clock()
        if at >= self.end:
            raise Refused("DEADLINE")
        return at

    def stop(self, at):
        self.fail("CHANNEL", at)
        if not self.stopped:
            self.stopped, self.intent = True, False
            self.kernel.disable()

    def control(self, body, at):
        if self.stopped or self.released or at >= self.end:
            raise Refused("STATE")
        if body.startswith("CLEAN|"):
            # A standalone build has exactly one cleanup broadcast. Its bound
            # is immutable: a second request is rejected, never parent-only min.
            if self.clean_seen:
                raise Refused("CLEAN_ALREADY_BOUND")
            self.end = min(self.end, integer(body[6:]), at + CAP)
            self.clean_seen = self.cleaning = True
        elif self.first is not None:
            return
        elif body == "PARK" and not (self.intent or self.parked or self.cleaning):
            self.intent = True
        elif body == "RELEASE" and self.gone:
            self.released = True
        else:
            raise Refused("STATE")

    def park(self, clock):
        self.check(clock)
        try:
            self.kernel.park()
        except OSError:
            self.fail("PARK", clock())
            self.check(clock)
            self.abandoned = True
            self.kernel.abandon()
            raise Refused("SELF_GROUP_UNPROVEN") from None
        self.parked = True

    def step(self, clock):
        at = clock()
        if not self.cleaning and at >= self.operation_end:
            self.fail("LIFETIME", at)
        self.check(clock)
        if not self.cleaning or self.gone:
            return False
        if not self.parked:
            self.park(clock)
        self.check(clock)
        if self.kernel.absent():
            self.kernel.disable()
            self.gone = True
            return True
        if self.term_end is None:
            at = self.check(clock)
            self.term_end = at + TERM
            self.kernel.send(signal.SIGTERM)
        elif not self.killed and clock() >= self.term_end:
            self.check(clock)
            self.killed = True
            self.kernel.send(signal.SIGKILL)
        return False

    def turn(self, channel, clock):
        self.check(clock)
        drained = False
        for _ in range(8):
            try:
                kind, body = channel.receive()
            except Refused:
                self.stop(clock())
                raise Refused(self.first) from None
            if kind == "frame":
                try:
                    self.control(body, clock())
                except Refused:
                    self.fail("CONTROL", clock())
            elif kind == "rejected":
                self.fail("CHANNEL", clock())
            elif kind == "yield":
                return False
            elif kind in ("eof", "quiescent"):
                drained = True
                break
            else:
                self.stop(clock())
                raise Refused(self.first)
        if channel.eof:
            if self.released and self.first is None:
                self.check(clock)
                return True
            self.fail("OWNER_EOF", clock())
        if not drained:
            return False
        if self.intent:
            self.intent = False
            if self.first is None:
                self.park(clock)
                self.check(clock)
                try:
                    channel.send("PARKED")
                except Refused:
                    self.fail("WRITE", clock())
        if self.step(clock) and self.first is None:
            channel.send("GONE")
        if self.gone and self.first:
            raise Refused(self.first)
        return False


class Kernel:
    def __init__(self, park):
        self.pid, self.destination = os.getpid(), park
        self.parked = self.disabled = False

    def park(self):
        if self.disabled:
            raise Refused("AUTHORITY")
        if not self.parked:
            os.setpgid(0, self.destination)
            self.parked = True

    def absent(self):
        if self.disabled or not self.parked:
            raise Refused("AUTHORITY")
        try:
            os.killpg(self.pid, 0)
            return False
        except ProcessLookupError:
            return True

    def send(self, signum):
        if self.disabled or not self.parked:
            raise Refused("AUTHORITY")
        try:
            os.killpg(self.pid, signum)
        except ProcessLookupError:
            pass

    def abandon(self):
        if self.disabled or self.parked or os.getpid() != self.pid or os.getpgrp() != self.pid:
            raise Refused("SELF_GROUP_AUTHORITY")
        self.disabled = True
        os.killpg(self.pid, signal.SIGKILL)
        raise Refused("SELF_GROUP_UNPROVEN")

    def disable(self):
        self.disabled = True


def private_dir(path, root):
    path = Path(path)
    if not path.is_absolute() or path.resolve(strict=True) != path or not path.is_relative_to(root):
        raise Refused("PRIVATE_PATH")
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise Refused("PRIVATE_MODE")
    return path


def file_hash(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as source:
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size > 128 * 1024 * 1024:
            raise Refused("HASH_FILE")
        digest = hashlib.sha256()
        count = 0
        while True:
            data = source.read(65536)
            if not data:
                break
            count += len(data)
            if count > 128 * 1024 * 1024:
                raise Refused("HASH_LIMIT")
            digest.update(data)
        after = os.fstat(source.fileno())
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
            raise Refused("HASH_DRIFT")
        return digest.hexdigest()


def spec_read(path):
    path = Path(path)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o600 or info.st_size > 65536:
        raise Refused("SPEC_FILE")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise Refused("SPEC_DUPLICATE")
            result[key] = value
        return result
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, "r", encoding="utf-8") as source:
        opened = os.fstat(source.fileno())
        if (opened.st_dev, opened.st_ino, opened.st_size) != (info.st_dev, info.st_ino, info.st_size):
            raise Refused("SPEC_DRIFT")
        spec = json.load(source, object_pairs_hook=unique)
    if type(spec) is not dict or set(spec) != {"root", "cwd", "argv", "env", "seconds", "image_sha256", "owner_sha256"}:
        raise Refused("SPEC_SCHEMA")
    root = Path(spec["root"])
    private_dir(root, root)
    for relative in ("home", "home/.codex", "home/.claude", "home/config", "home/state", "home/capture", "home/AppData", "tmp"):
        private_dir(root / relative, root)
    argv = spec["argv"]
    if type(argv) is not list or not 1 <= len(argv) <= 64 or any(type(v) is not str or not v or "\0" in v or len(v) > 4096 for v in argv):
        raise Refused("ARGV")
    image = Path(argv[0])
    if not image.is_absolute() or image.resolve(strict=True) != image or not image.is_file() or not os.access(image, os.X_OK):
        raise Refused("IMAGE")
    if image.name != "cargo":
        raise Refused("BUILD_IMAGE")
    if file_hash(image) != spec["image_sha256"] or file_hash(Path(__file__).resolve()) != spec["owner_sha256"]:
        raise Refused("SOURCE_BINDING")
    cwd = Path(spec["cwd"])
    if not cwd.is_absolute() or cwd.resolve(strict=True) != cwd or not cwd.is_dir():
        raise Refused("CWD")
    if type(spec["seconds"]) is not int or not 3 <= spec["seconds"] <= 600:
        raise Refused("LIFETIME")
    env = spec["env"]
    allowed = {"HOME", "CODEX_HOME", "CLAUDE_CONFIG_DIR", "KEITO_CAPTURE_HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "APPDATA", "TMPDIR", "PATH", "CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR", "CARGO_BUILD_JOBS", "CARGO_NET_OFFLINE", "RUST_TEST_THREADS", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM", "GIT_CONFIG_NOSYSTEM"}
    if type(env) is not dict or set(env) != allowed or any(type(v) is not str or not v or "\0" in v or len(v) > 4096 for v in env.values()):
        raise Refused("ENV")
    for key, relative in (("HOME", "home"), ("CODEX_HOME", "home/.codex"), ("CLAUDE_CONFIG_DIR", "home/.claude"), ("KEITO_CAPTURE_HOME", "home/capture"), ("XDG_CONFIG_HOME", "home/config"), ("XDG_STATE_HOME", "home/state"), ("APPDATA", "home/AppData"), ("TMPDIR", "tmp")):
        if env[key] != str(root / relative):
            raise Refused("PRIVATE_ENV")
    if env["PATH"] != ":".join((str(image.parent), "/usr/bin", "/bin", "/usr/sbin", "/sbin")):
        raise Refused("TOOLCHAIN_PATH")
    if env["CARGO_NET_OFFLINE"] != "true" or env["CARGO_BUILD_JOBS"] != "2" or env["RUST_TEST_THREADS"] != "1" or env["GIT_CONFIG_GLOBAL"] != "/dev/null" or env["GIT_CONFIG_SYSTEM"] != "/dev/null" or env["GIT_CONFIG_NOSYSTEM"] != "1":
        raise Refused("BUILD_ENV")
    # Only the caller's exact recorded toolchain/cache paths, never PATH lookup.
    for key in ("CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR"):
        candidate = Path(env[key])
        if not candidate.is_absolute() or candidate.resolve(strict=True) != candidate or not candidate.is_dir():
            raise Refused("BUILD_PATH")
    if not Path(env["CARGO_TARGET_DIR"]).is_relative_to(root):
        raise Refused("PRIVATE_TARGET")
    return spec


def exclusive(path):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    return os.fdopen(fd, "w", encoding="utf-8")


class BuildRun:
    def __init__(self, root, hard):
        self.root, self.hard = root, hard
        self.keeper = self.child = self.keeper_status = self.child_status = None
        self.child_attempted = False
        self.first = None
        self.secondary = []
        self.cancelled = False
        self.end = None
        self.gone = self.release = self.proof = False
        self.channel = None
        self.writer_attempted = self.writer_shutdown_ok = False
        self.trace = exclusive(root / "events.jsonl")
        self.output = exclusive(root / "output.log")

    def fail(self, reason):
        if self.first is None:
            self.first = reason
        elif reason != self.first and reason not in self.secondary and len(self.secondary) < 8:
            self.secondary.append(reason)

    def note_cancelled(self):
        if getattr(self, "cancelled", False):
            self.fail("CANCELLED")

    def checkpoint(self):
        self.note_cancelled()
        if getattr(self, "cancelled", False):
            raise Refused(self.first or "CANCELLED")

    def event(self, tag):
        try:
            self.trace.write(json.dumps({"event": tag, "at": time.monotonic_ns()}) + "\n")
            self.trace.flush()
            os.fsync(self.trace.fileno())
        except OSError:
            raise Refused("RECEIPT") from None
        self.checkpoint()

    def collect(self):
        self.note_cancelled()
        if self.child is not None and self.child_status is None:
            try:
                self.child_status = self.child.poll()
            except BaseException:
                self.fail("CHILD_WAIT")
        if self.keeper is not None and self.keeper_status is None:
            try:
                pid, status = os.waitpid(self.keeper, os.WNOHANG)
                if pid:
                    self.keeper_status = os.waitstatus_to_exitcode(status)
            except BaseException:
                self.fail("KEEPER_WAIT")
        if self.keeper_status is not None and not self.release:
            self.fail("KEEPER_EXIT_EARLY")

    def all_waited(self):
        return (self.child is None or self.child_status is not None) and (self.keeper is None or self.keeper_status is not None)

    def close_writer(self):
        if self.channel and not self.writer_attempted:
            self.writer_attempted = True
            try:
                self.channel.stream.shutdown(socket.SHUT_WR)
                self.writer_shutdown_ok = True
            except BaseException:
                self.fail("SHUTDOWN")
                try:
                    self.channel.stream.close()
                except BaseException:
                    self.fail("SHUTDOWN_CLOSE")

    def wait_frame(self, body, end):
        while time.monotonic_ns() < end:
            self.checkpoint()
            kind, value = self.channel.receive()
            if kind == "frame":
                if value != body:
                    raise Refused("HANDSHAKE")
                if time.monotonic_ns() >= end:
                    raise Refused("HANDSHAKE_LATE")
                self.checkpoint()
                return
            if kind in ("eof", "rejected"):
                raise Refused("HANDSHAKE")
            time.sleep(0.001)
        raise Refused("HANDSHAKE_DEADLINE")

    def begin(self):
        self.note_cancelled()
        # Single immutable broadcast, before cancellation wait collection.
        if self.end is not None:
            raise Refused("CLEAN_ALREADY_BOUND")
        self.end = min(self.hard, time.monotonic_ns() + CAP)
        self.channel.send("CLEAN|" + str(self.end))
        if self.first:
            self.close_writer()

    def settle(self):
        while True:
            self.collect()  # Available wait facts survive every previous fault.
            if self.first:
                self.close_writer()
            at = time.monotonic_ns()
            if at >= self.end:
                self.fail("CLEANUP_DEADLINE")
                break
            try:
                kind, body = self.channel.receive()
                if kind == "rejected":
                    raise Refused("CHANNEL")
                if kind == "frame":
                    if self.first or body != "GONE" or self.gone:
                        raise Refused("TERMINAL")
                    self.gone = True
                if self.gone and not self.release and self.child_status is not None and not self.first:
                    if time.monotonic_ns() >= self.end:
                        raise Refused("CLEANUP_DEADLINE")
                    self.channel.send("RELEASE")
                    self.close_writer()
                    self.release = True
                if kind == "eof":
                    if not self.release:
                        self.fail("EARLY_EOF")
                    if self.all_waited():
                        if self.keeper_status != 0:
                            self.fail("KEEPER_FAILED")
                        self.proof = not self.first and self.gone and self.release and not self.channel.pending and time.monotonic_ns() < self.end
                        break
            except (Refused, OSError) as error:
                self.fail(str(error) if isinstance(error, Refused) else "CHANNEL_IO")
                self.close_writer()
            time.sleep(0.001)
        self.collect()

    def retain_waits(self):
        # After a failed proof deadline, only immediately available direct wait
        # collection continues, inside the ORIGINAL immutable operation lifetime.
        # No new signalling, retry budget, or proof upgrade occurs here.
        while not self.all_waited() and time.monotonic_ns() < self.hard:
            self.collect()
            time.sleep(0.001)
        self.collect()

    def cancel_signal(self, _signum, _frame):
        # Existing-key, fixed-value latch only. No exception, I/O, or waits in a
        # handler; explicit normal-work checkpoints stop later admissions.
        self.cancelled = True

    def guard(self, reason, action):
        try:
            return True, action()
        except BaseException:
            self.fail(reason)
            return False, None

    def finalize(self, parent, child, result_saved, saved_handlers, cutpoint=None):
        self.note_cancelled()
        # Freeze pre-fallback facts even if FIRST publication failed. facts()
        # copies its secondary list, so late observations cannot mutate it.
        if cutpoint is None:
            cutpoint = self.facts()
        self.guard("FINAL_SHUTDOWN", self.close_writer)
        if child is not None:
            self.guard("CHILD_END_CLOSE", child.close)
        if not result_saved:
            self.proof = False
        self.guard("AVAILABLE_WAITS", self.collect)
        self.guard("FINAL_WAIT", self.retain_waits)
        self.note_cancelled()
        after = self.facts()
        after.update(first_preserved=result_saved, first_result_write_failed=not result_saved,
                     cutpoint=cutpoint, handler_restoration_pending=True)
        def save():
            with exclusive(self.root / "FINAL.json") as output:
                json.dump(after, output, indent=2)
                output.write("\n")
                output.flush()
                os.fsync(output.fileno())
        final_saved, _ = self.guard("FINAL_RECEIPT", save)
        if parent is not None:
            self.guard("PARENT_END_CLOSE", parent.close)
        self.guard("OUTPUT_CLOSE", self.output.close)
        self.guard("TRACE_CLOSE", self.trace.close)
        self.note_cancelled()
        def publish(pending):
            packet = json.dumps({"owner_result": self.facts(), "cutpoint": cutpoint,
                                 "first_saved": result_saved, "final_saved": final_saved,
                                 "handler_restoration_pending": pending})
            if len(packet.encode("utf-8")) > 16384:
                raise Refused("FACT_LIMIT")
            ok, _ = self.guard("FACT_STDOUT", lambda: print(packet, flush=True))
            if not ok:
                # A second catchable signal/output failure cannot skip an
                # alternate bounded fact sink or handler restoration.
                packet = json.dumps({"owner_result": self.facts(), "cutpoint": cutpoint,
                                     "first_saved": result_saved, "final_saved": final_saved,
                                     "handler_restoration_pending": pending})
                self.guard("FACT_STDERR", lambda: print(packet, file=sys.stderr, flush=True))
        self.guard("FACT_PUBLICATION", lambda: publish(True))
        # Only the final handler handoff is masked, after waits/closes/facts;
        # the long bounded cleanup itself uses the non-throwing latch.
        masked, previous_mask = self.guard("HANDOFF_MASK", lambda: signal.pthread_sigmask(
            signal.SIG_BLOCK, (signal.SIGINT, signal.SIGTERM)))
        restored = False
        if masked:
            try:
                ok, pending = self.guard("PENDING_CANCEL", signal.sigpending)
                if ok and pending.intersection((signal.SIGINT, signal.SIGTERM)):
                    self.cancel_signal(None, None)
                restored = True
                for signum, handler in saved_handlers.items():
                    ok, _ = self.guard("RESTORE_HANDLER", lambda s=signum, h=handler: signal.signal(s, h))
                    restored = restored and ok
                self.note_cancelled()
                self.guard("FINAL_FACT_PUBLICATION", lambda: publish(not restored))
            finally:
                self.guard("HANDOFF_UNMASK", lambda: signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask))
        else:
            # Leave this standalone helper's latch installed until failed exit,
            # rather than restoring defaults during an unprotected handoff.
            self.guard("FINAL_FACT_PUBLICATION", lambda: publish(True))

    def facts(self):
        return {"first_fault": self.first, "secondary_faults": list(self.secondary),
                "cancelled": getattr(self, "cancelled", False),
                "keeper_spawned": self.keeper is not None, "child_spawned": self.child is not None,
                "keeper_pid": self.keeper, "child_pid": getattr(self.child, "pid", None),
                "child_spawn_attempted": self.child_attempted,
                "missing_handle_after_attempt_is_unknown": self.child_attempted and self.child is None,
                "keeper_wait": self.keeper_status, "child_wait": self.child_status,
                "group_absence_asserted_by_keeper": self.gone,
                "raw_esrch_independently_observed": False,
                "release_sent": self.release, "writer_shutdown_ok": self.writer_shutdown_ok, "terminal_eof": bool(self.channel and self.channel.eof and not self.channel.pending),
                "cleanup_proof": self.proof, "hard_end": self.hard, "cleanup_end": self.end}


def child_keeper(stream, other, nonce, park, hard, maxfd, inherited_mask):
    other.close()
    try:
        # External cancellation of the owner's group must close the lifeline,
        # not kill its parked keeper before it can clean the reserved group.
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.pthread_sigmask(signal.SIG_SETMASK, inherited_mask)
        os.setpgid(0, 0)
        fd = stream.fileno()
        null = os.open("/dev/null", os.O_RDWR)
        for target in (0, 1, 2):
            os.dup2(null, target)
        if null > 2 and null != fd:
            os.close(null)
        os.closerange(3, fd)
        os.closerange(fd + 1, maxfd)
        channel = Channel(stream, nonce)
        machine = Keeper(Kernel(park), hard)
        channel.send("READY")
        while not machine.turn(channel, time.monotonic_ns):
            time.sleep(0.001)
        os._exit(0)
    except BaseException:
        os._exit(1)


def run(spec):
    if os.environ.get("KEITO_BOOTSTRAP_POLICY_CONFIRMED") != "source-group-build-only":
        raise Refused("POLICY_PREREQUISITE")
    root = Path(spec["root"])
    hard = time.monotonic_ns() + spec["seconds"] * 1_000_000_000
    state = BuildRun(root, hard)
    parent = child = None
    first_facts = None
    result_saved = False
    saved_handlers = {}
    try:
        for signum in (signal.SIGINT, signal.SIGTERM):
            saved_handlers[signum] = signal.signal(signum, state.cancel_signal)
        parent, child = socket.socketpair()
        nonce = os.urandom(16).hex()
        state.channel = Channel(parent, nonce)  # Before fork: no lost child on setup error.
        maxfd = resource.getrlimit(resource.RLIMIT_NOFILE)[0]
        if not 3 <= maxfd <= 1_048_576:
            raise Refused("FD_BOUND")
        park = os.getpgrp()
        # A caught cancellation cannot interrupt fork/returned-handle assignment.
        # Uncatchable parent death cannot supply direct waits and is not proof.
        mask = signal.pthread_sigmask(signal.SIG_BLOCK, (signal.SIGINT, signal.SIGTERM))
        try:
            pid = os.fork()
            if pid == 0:
                child_keeper(child, parent, nonce, park, hard, maxfd, mask)
            state.keeper = pid  # BEFORE unmasking or any READY/receipt I/O.
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, mask)
        child.close()
        child = None
        try:
            state.event("KEEPER_REGISTERED")
            state.wait_frame("READY", min(hard - CAP, time.monotonic_ns() + CAP))
            state.checkpoint()
            mask = signal.pthread_sigmask(signal.SIG_BLOCK, (signal.SIGINT, signal.SIGTERM))
            try:
                state.checkpoint()
                state.child_attempted = True
                def join_group():
                    os.setpgid(0, pid)
                    signal.pthread_sigmask(signal.SIG_SETMASK, mask)
                state.child = subprocess.Popen(spec["argv"], cwd=spec["cwd"], env=spec["env"],
                                               stdin=subprocess.DEVNULL, stdout=state.output,
                                               stderr=state.output, close_fds=True,
                                               preexec_fn=join_group)
            except OSError:
                state.fail("SPAWN")
                raise Refused("SPAWN") from None
            finally:
                signal.pthread_sigmask(signal.SIG_SETMASK, mask)
            state.event("CHILD_REGISTERED")
            state.channel.send("PARK")
            state.wait_frame("PARKED", min(hard - CAP, time.monotonic_ns() + CAP))
            # Normal completion is not group proof. A collected leader still
            # requires CLEAN, keeper absence assertion and clean terminal EOF.
            while state.child_status is None and time.monotonic_ns() < hard - CAP:
                state.collect()
                if state.first:
                    raise Refused(state.first)
                time.sleep(0.001)
            if state.child_status is None:
                raise Refused("BUILD_LIFETIME")
        except (Refused, OSError) as error:
            state.fail(str(error) if isinstance(error, Refused) else "OWNER_IO")
        try:
            state.begin()
        except (Refused, OSError):
            state.fail("BEGIN")
            state.end = min(hard, time.monotonic_ns() + CAP) if state.end is None else state.end
            state.close_writer()
        state.settle()
        first_facts = state.facts()
        first_facts["command_success"] = state.child_status == 0
        try:
            with exclusive(root / "FIRST.json") as output:
                json.dump(first_facts, output, indent=2)
                output.write("\n")
                output.flush()
                os.fsync(output.fileno())
        except OSError:
            raise Refused("FIRST_RECEIPT") from None
        result_saved = True
    except BaseException as error:
        # Receipt/setup failures cannot discard registered handles or replace
        # the first reason with a later success. No raw exception text escapes.
        state.fail(str(error) if isinstance(error, Refused) else "OWNER_INTERNAL")
    finally:
        state.finalize(parent, child, result_saved, saved_handlers, first_facts)
    return bool(result_saved and first_facts["cleanup_proof"] and first_facts["command_success"]
                and not state.first and state.all_waited())


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2:
            raise Refused("ARGS")
        prepared = spec_read(sys.argv[1])
    except BaseException as error:
        code = str(error) if isinstance(error, Refused) else "SETUP"
        print(json.dumps({"setup_refused": code, "actors_started": False}), flush=True)
        sys.exit(1)
    try:
        success = run(prepared)
    except BaseException:
        # Do not call a post-entry error a preflight refusal or fabricate waits.
        print(json.dumps({"owner_entry_failed": True, "cleanup_qualified": False}), flush=True)
        success = False
    sys.exit(0 if success else 1)
