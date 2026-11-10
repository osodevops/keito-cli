"""Developer-only tracked direct children; no process discovery or product authority."""
import datetime
import hashlib
import json
import os
import secrets
import signal
import subprocess
import time


class Cancelled(BaseException):
    def __init__(self, signum):
        self.signum = signum


class ChildFailure(RuntimeError):
    pass


class LegacyDirectChildren:
    """Historical direct/group tests only; never the actual capture runner.

    Preserved to keep its original regression assertions and failed history.
    It does not supply the new cooperative descendant proof.
    """
    def __init__(self, root, environment, cwd):
        self.root, self.environment, self.cwd = root, environment, cwd
        self.active = None
        self.cleaning = False
        self.spawning = False
        self.pending_cancel = None
        self.handlers = {}

    def __enter__(self):
        for signum in (signal.SIGTERM, signal.SIGINT):
            self.handlers[signum] = signal.signal(signum, self._cancel)
        return self

    def _cancel(self, signum, _frame):
        if self.cleaning:
            return
        if self.spawning:
            self.pending_cancel = signum
            return
        raise Cancelled(signum)

    def __exit__(self, *_):
        try:
            if self.active is not None and "cleanup" not in self.active:
                self._cleanup(self.active)
        finally:
            for signum, handler in self.handlers.items():
                signal.signal(signum, handler)

    def _record(self, name, value):
        with (self.root / name).open("x") as output:
            json.dump(value, output, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())

    def _cleanup(self, active):
        self.cleaning = True
        child = active["child"]
        actions = []
        try:
            # The Popen handle is still owned and unreaped. Its PID cannot be
            # recycled before wait; never signal a reaped PID or discover others.
            if child.returncode is None:
                for signum, grace in ((signal.SIGTERM, 2), (signal.SIGKILL, 2)):
                    try:
                        os.killpg(child.pid, signum)
                        actions.append({"signal": signum, "owned_pgid": child.pid})
                    except OSError as failure:
                        actions.append({"signal": signum, "signal_errno": failure.errno})
                    try:
                        child.wait(timeout=grace)
                        break
                    except subprocess.TimeoutExpired:
                        continue
                    except OSError as failure:
                        actions.append({"wait_errno": failure.errno})
                        continue
            waited = child.returncode is not None
            active["cleanup"] = {"actions": actions, "waited": waited, "returncode": child.returncode}
            if waited:
                self.active = None
            return waited
        finally:
            self.cleaning = False

    def run(self, label, arguments, timeout, extra=None, capture_limit=0):
        if self.active is not None or not label or any(not (value.isalnum() or value == "-") for value in label):
            raise ChildFailure("invalid owned child operation")
        started = time.monotonic()
        active = None
        error = None
        waited = False
        with (self.root / (label + ".log")).open("xb") as output, (self.root / (label + ".stderr.log")).open("xb") as errors:
            try:
                # Defer the handler only over spawn+assignment. An inherited
                # OS signal mask would also block TERM in the executed child.
                self.pending_cancel = None
                self.spawning = True
                try:
                    child = subprocess.Popen(arguments, cwd=self.cwd,
                                             env={**self.environment, **(extra or {})}, stdin=subprocess.DEVNULL,
                                             stdout=output, stderr=errors, start_new_session=True)
                    active = {"child": child, "pid": child.pid, "pgid": child.pid,
                              "launch_before_monotonic_ns": int(started * 1_000_000_000)}
                    self.active = active
                finally:
                    self.spawning = False
                if self.pending_cancel is not None:
                    raise Cancelled(self.pending_cancel)
                active.update({"launch_nonce": secrets.token_hex(16), "launch_after_monotonic_ns": time.monotonic_ns(),
                               "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                               "observed_pgid": os.getpgid(child.pid)})
                if active["observed_pgid"] != child.pid:
                    raise ChildFailure("owned child process group mismatch")
                self._record(label + "-child-started.json", {key: value for key, value in active.items() if key != "child"})
                child.wait(timeout=timeout)
                waited = True
                self.active = None  # Only after the confirmed direct wait.
            except BaseException as failure:
                error = failure
            finally:
                if active is not None and not waited:
                    waited = self._cleanup(active)
        result = {"source_fixture_only": True, "command": arguments, "timeout_seconds": timeout,
                  "elapsed_seconds": time.monotonic() - started, "waited": waited,
                  "returncode": active["child"].returncode if active else None,
                  "child": {key: value for key, value in active.items() if key != "child"} if active else None,
                  "error_type": type(error).__name__ if error else None,
                  "cancel_signal": error.signum if isinstance(error, Cancelled) else None,
                  "timed_out": isinstance(error, subprocess.TimeoutExpired)}
        try:
            for suffix in (".log", ".stderr.log"):
                with (self.root / (label + suffix)).open("rb") as source:
                    digest = hashlib.sha256()
                    for block in iter(lambda: source.read(64 * 1024), b""):
                        digest.update(block)
                result[suffix + "_sha256"] = digest.hexdigest()
            self._record(label + "-result.json", result)
        except BaseException as receipt_error:
            # Cleanup and the original failure must survive a receipt fault.
            if error is None:
                error = receipt_error
            try:
                LegacyDirectChildren._record(self, label + "-cleanup-fallback.json", result)
            except BaseException:
                pass
        if error is not None:
            raise error
        if not waited or result["returncode"] != 0:
            raise ChildFailure("owned source child failed; preserve the first result")
        if capture_limit:
            path = self.root / (label + ".log")
            if path.stat().st_size > capture_limit:
                raise ChildFailure("owned metadata output exceeds bound")
            return path.read_bytes()
        return None


class OwnedChildren:
    """Retained multi-slot COOPERATIVE runtime children, not compiler authority.

    No PID/group signalling and no raw output files. A terminal is accepted only
    with actual read-zero EOF on each pipe/socket and the retained direct wait.
    """
    def __init__(self, root, environment, cwd, owner=None):
        from source_qa_protocol import SourceOwner
        self.root, self.environment, self.cwd = root, environment, cwd
        self.owner = owner if owner is not None else SourceOwner()
        self.owns_owner = owner is None
        self.slots = []
        self.handlers = {}
        self.spawning = False
        self.pending_cancel = None
        self.cleanup_end = None
        self.io_reserved = 0
        self.first_error = None
        self.entered = False

    @property
    def active(self):
        return next((slot for slot in self.slots if not slot["collected"]), None)

    def _record(self, name, value):
        # Receipt contents are bounded metadata, never child input/output.
        with (self.root / name).open("x") as output:
            json.dump(value, output, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())

    def _fail(self, error, reason):
        if self.first_error is None:
            self.first_error = error
        self.owner.fail(reason)

    def _cancel(self, signum, _frame):
        self.pending_cancel = signum
        if not self.spawning:
            self.owner.fail("RUNTIME_SIGNAL")

    def __enter__(self):
        if self.entered:
            raise ChildFailure("runtime manager reused")
        self.entered = True
        if self.owns_owner:
            self.owner.__enter__()
        self.owner.register_manager(self)
        for signum in (signal.SIGTERM, signal.SIGINT):
            self.handlers[signum] = signal.signal(signum, self._cancel)
        return self

    def __exit__(self, kind, exception, traceback):
        if exception is not None:
            self._fail(exception, "RUNTIME_OPERATION_OR_RECEIPT")
        try:
            if self.owns_owner:
                return self.owner.__exit__(kind, exception, traceback)
            self.begin_cleanup(time.monotonic_ns() + 4_000_000_000)
            while not self.poll_cleanup(self.cleanup_end):
                if time.monotonic_ns() >= self.cleanup_end:
                    self.owner.fail("RUNTIME_CLEANUP_DEADLINE")
                    break
                time.sleep(0.001)
        finally:
            for signum, handler in self.handlers.items():
                signal.signal(signum, handler)

    def start(self, label, arguments, timeout, extra=None, capture_limit=0, input_bytes=b""):
        import socket
        import threading
        if (not self.entered or self.cleanup_end is not None or len(self.slots) >= 4096
                or not label or any(not (c.isalnum() or c == "-") for c in label)
                or any(slot["label"] == label for slot in self.slots)
                or type(timeout) not in (int, float) or not 0 < timeout <= 600
                or type(capture_limit) is not int or not 0 <= capture_limit <= 2 * 1024 * 1024
                or type(input_bytes) is not bytes or len(input_bytes) > 1024 * 1024
                or threading.active_count() != 1):
            raise ChildFailure("invalid cooperative runtime operation")
        self.owner.check()
        at = time.monotonic_ns()
        end = at + int(timeout * 1_000_000_000)
        if self.owner.deadline_ns is not None:
            end = min(end, self.owner.deadline_ns)
        end = min(end, at + 4_000_000_000)
        if end <= at:
            raise ChildFailure("expired cooperative operation")
        charge = len(input_bytes) + capture_limit
        if self.io_reserved + charge > 4 * 1024 * 1024:
            raise ChildFailure("runtime buffer budget")
        nonce = secrets.token_hex(16)
        parent, child_end = socket.socketpair()
        endpoint = self.owner.endpoint(parent, nonce)
        slot = {"label": label, "nonce": nonce, "child": None, "endpoint": endpoint,
                "end": end, "started": at, "input": input_bytes, "offset": 0,
                "capture_limit": capture_limit, "stdout": bytearray(), "stdout_count": 0,
                "stderr_count": 0, "stdout_eof": False, "stderr_eof": False,
                "cancel_sent": False, "waited_published": False, "collected": False,
                "accepted": None, "retrieved": False, "first_fault": None}
        # Reserve history and buffers before Popen. Slot remains even on error.
        self.slots.append(slot)
        self.io_reserved += charge
        env = {**self.environment, **(extra or {})}
        for key in ("KEITO_SOURCE_OWNER_FD", "KEITO_SOURCE_OWNER_NONCE"):
            env.pop(key, None)
        env.update({"KEITO_PROCESS_OWNER_FD": "4", "KEITO_PROCESS_OWNER_NONCE": nonce,
                    "KEITO_PROCESS_OWNER_PID": str(os.getpid()), "KEITO_PROCESS_OWNER_END_NS": str(end),
                    "KEITO_PROCESS_OWNER_CONTRACT": "2"})
        # Keep FD4 valid for pass_fds, then map the private endpoint in the
        # single-threaded child callback. No child inherits the blocked mask.
        placeholder = None
        try:
            try:
                os.fstat(4)
            except OSError:
                placeholder = os.open("/dev/null", os.O_RDONLY)
                if placeholder != 4:
                    os.dup2(placeholder, 4, inheritable=False)
                    os.close(placeholder)
                    placeholder = 4
            source_fd = child_end.fileno()
            def attach():
                os.dup2(source_fd, 4, inheritable=True)
                if source_fd != 4:
                    os.close(source_fd)
            self.spawning = True
            try:
                child = subprocess.Popen(arguments, cwd=self.cwd, env=env,
                                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                         close_fds=True, pass_fds=tuple(sorted({4, source_fd})), preexec_fn=attach)
                slot["child"] = child  # BEFORE flags, endpoint identity or receipt.
            finally:
                self.spawning = False
            endpoint.expected_pid = child.pid
            if self.pending_cancel is not None:
                raise Cancelled(self.pending_cancel)
            for pipe in (child.stdin, child.stdout, child.stderr):
                os.set_blocking(pipe.fileno(), False)
            self.owner.check()
            self.owner.forward([f"REGISTER|{os.getpid()}|{child.pid}|{nonce}|0"])
            # Ordinary exclusive receipt, after actual returned-handle retention.
            self._record(label + "-child-started.json", {"pid": child.pid, "launch_nonce": nonce,
                         "launch_before_monotonic_ns": at, "launch_after_monotonic_ns": time.monotonic_ns()})
            return slot
        except BaseException as error:
            self._fail(error, "RUNTIME_START_OR_RECEIPT")
            self.begin_cleanup(min(end, time.monotonic_ns() + 4_000_000_000))
            raise
        finally:
            child_end.close()
            if placeholder is not None:
                os.close(placeholder)

    def _poll_slot(self, slot, accepting):
        from source_qa_protocol import ProtocolError
        child = slot["child"]
        if child is None:
            slot["collected"] = True
            return
        endpoint = slot["endpoint"]
        try:
            endpoint.receive(parent=False)
        except ProtocolError as error:
            slot["first_fault"] = slot["first_fault"] or str(error)
            self._fail(error, "RUNTIME_ENDPOINT")
        events, endpoint.events = endpoint.events, []
        if events:
            try:
                self.owner.forward(events)
            except BaseException as error:
                self._fail(error, "RUNTIME_FORWARD")
        if child.stdin is not None:
            if self.cleanup_end is not None or slot["offset"] == len(slot["input"]):
                child.stdin.close(); child.stdin = None
            else:
                try:
                    count = os.write(child.stdin.fileno(), memoryview(slot["input"])[slot["offset"]:slot["offset"] + 4096])
                    if count <= 0:
                        raise ChildFailure("runtime input short write")
                    slot["offset"] += count
                except (BlockingIOError, InterruptedError):
                    pass
                except OSError as error:
                    slot["first_fault"] = slot["first_fault"] or "RUNTIME_INPUT_WRITE"
                    self._fail(error, "RUNTIME_INPUT_WRITE")
                    child.stdin.close(); child.stdin = None
        for name in ("stdout", "stderr"):
            pipe = getattr(child, name)
            if pipe is None:
                continue
            for _ in range(8):
                try:
                    block = os.read(pipe.fileno(), 4096)
                except (BlockingIOError, InterruptedError):
                    break
                except OSError as error:
                    slot["first_fault"] = slot["first_fault"] or "RUNTIME_OUTPUT_READ"
                    self._fail(error, "RUNTIME_OUTPUT_READ")
                    break
                if not block:
                    slot[name + "_eof"] = True
                    pipe.close(); setattr(child, name, None)
                    break
                slot[name + "_count"] += len(block)
                if name == "stdout":
                    remaining = max(0, slot["capture_limit"] - len(slot["stdout"]))
                    slot["stdout"].extend(block[:remaining])
                if slot[name + "_count"] > (slot["capture_limit"] if name == "stdout" else 0):
                    slot["first_fault"] = slot["first_fault"] or "RUNTIME_OUTPUT_LIMIT"
                    self._fail(ChildFailure("runtime output limit"), "RUNTIME_OUTPUT_LIMIT")
        try:
            status = child.poll()
        except OSError as error:
            status = None
            self._fail(error, "RUNTIME_DIRECT_WAIT")
        if status is not None and not slot["waited_published"] and (endpoint.settled or endpoint.first_fault):
            slot["waited_published"] = True
            try:
                self.owner.forward([f"WAITED|{os.getpid()}|{child.pid}|{slot['nonce']}|{status}"])
            except BaseException as error:
                self._fail(error, "RUNTIME_WAIT_RECEIPT")
        slot["collected"] = (status is not None and slot["stdout_eof"] and slot["stderr_eof"]
                             and child.stdin is None and (endpoint.eof or endpoint.first_fault is not None))
        if (accepting and slot["accepted"] is None and slot["collected"] and endpoint.settled
                and slot["first_fault"] is None and self.owner.first_fault is None
                and time.monotonic_ns() < slot["end"]):
            slot["accepted"] = time.monotonic_ns()

    def poll(self):
        try:
            self.owner.check()
        except BaseException as error:
            self._fail(error, "RUNTIME_PARENT_OR_SIGNAL")
        for slot in self.slots:
            if not slot["collected"]:
                self._poll_slot(slot, True)
            if slot["accepted"] is None and time.monotonic_ns() >= slot["end"]:
                self._fail(subprocess.TimeoutExpired(slot["label"], 0), "RUNTIME_DEADLINE")
        if self.owner.first_fault:
            self.begin_cleanup(time.monotonic_ns() + 4_000_000_000)
        return all(slot["accepted"] is not None for slot in self.slots)

    def begin_cleanup(self, end):
        self.cleanup_end = min(self.cleanup_end or end, end, time.monotonic_ns() + 4_000_000_000)
        if self.owner.deadline_ns is not None:
            self.cleanup_end = min(self.cleanup_end, self.owner.deadline_ns)
        # Broadcast ALL cancellation/close intents before any wait or drain.
        for slot in self.slots:
            if slot["child"] is None or slot["accepted"] is not None:
                continue
            child = slot["child"]
            if child.stdin is not None:
                child.stdin.close(); child.stdin = None
            if not slot["cancel_sent"] and not slot["endpoint"].settled:
                slot["cancel_sent"] = True
                try:
                    slot["endpoint"].send("CANCEL", deadline_ns=max(1, self.cleanup_end))
                except BaseException as error:
                    self._fail(error, "RUNTIME_CANCEL_WRITE")

    def poll_cleanup(self, end):
        self.cleanup_end = min(self.cleanup_end or end, end)
        for slot in self.slots:
            if not slot["collected"]:
                self._poll_slot(slot, False)
        return all(slot["collected"] for slot in self.slots)

    def complete(self, slot):
        if not any(value is slot for value in self.slots) or slot["retrieved"]:
            raise ChildFailure("runtime slot identity")
        while slot["accepted"] is None:
            self.poll()
            if self.first_error is not None:
                raise self.first_error
            time.sleep(0.001)
        slot["retrieved"] = True
        child = slot["child"]
        result = {"source_fixture_only": True, "pid": child.pid, "waited": child.returncode is not None,
                  "returncode": child.returncode, "terminal_eof": slot["endpoint"].settled,
                  "stdout_eof": slot["stdout_eof"], "stderr_eof": slot["stderr_eof"],
                  "stdout_bytes": slot["stdout_count"], "stderr_bytes": slot["stderr_count"],
                  "accepted_monotonic_ns": slot["accepted"], "deadline_ns": slot["end"]}
        try:
            self._record(slot["label"] + "-result.json", result)
        except BaseException as error:
            self._fail(error, "RUNTIME_RESULT_RECEIPT")
            self.begin_cleanup(time.monotonic_ns() + 4_000_000_000)
            raise
        if child.returncode != 0:
            error = ChildFailure("cooperative runtime child failed")
            self._fail(error, "RUNTIME_COMMAND_FAILED")
            raise error
        return bytes(slot["stdout"]) if slot["capture_limit"] else None

    def wait_result(self, slot):
        """Expected business nonzero is returned only after ownership proof."""
        try:
            output = self.complete(slot)
        except ChildFailure:
            if (slot["accepted"] is None or slot["first_fault"] is not None
                    or self.owner.first_fault != "RUNTIME_COMMAND_FAILED"):
                raise
            # A nonzero command is still an operation failure for this owner;
            # callers inspecting expected refusal must use a separate owner.
            output = bytes(slot["stdout"]) if slot["capture_limit"] else b""
        return subprocess.CompletedProcess([], slot["child"].returncode, output or b"", b"")

    def run(self, label, arguments, timeout, extra=None, capture_limit=0, input_bytes=b""):
        return self.complete(self.start(label, arguments, timeout, extra, capture_limit, input_bytes))
