"""Developer-only tracked direct children; no process discovery or product authority."""
import datetime
import hashlib
import json
import os
import secrets
import signal
import subprocess
import time
from pathlib import Path


# One original installation maximum; inherited parents always clip it.
INSTALL_OPERATION_NS = 30_000_000_000


def _stderr_category(block):
    """Closed diagnostic from an already-read block; never retain its content."""
    if type(block) is not bytes or len(block) > 4096:
        return None
    try:
        value = json.loads(block)
    except (ValueError, UnicodeError, RecursionError):
        return None
    if (type(value) is not dict or value.get("error") is not True
            or not set(value) <= {"error", "code", "message", "suggestion", "details"}
            or type(value.get("code")) is not int or not 1 <= value["code"] <= 8):
        return None
    if value["code"] == 8:
        messages = {
            "Configuration error: Repository command ownership unavailable; capture paused": "REPOSITORY_OWNERSHIP",
            "Configuration error: Capture command ownership unavailable; collection remains paused": "COMMAND_OWNERSHIP",
            "Configuration error: Command ownership did not settle; collection remains paused": "TERMINAL_SETTLEMENT",
            "Configuration error: Numeric profile qualification is unavailable; review the supported native version, managed installation and explicit profile approval. Metadata capture remains available.": "PROFILE_QUALIFICATION",
        }
        message = value.get("message")
        if type(message) is str and message in messages:
            return messages[message]
        reasons = {
            "admission_refused": ("ownership admission refused", "VERSION_PROBE_ADMISSION"),
            "deadline": ("original deadline exceeded", "VERSION_PROBE_DEADLINE"),
            "clock": ("monotonic clock unavailable", "VERSION_PROBE_CLOCK"),
            "ownership": ("ownership proof unavailable", "VERSION_PROBE_OWNERSHIP"),
            "unavailable": ("probe unavailable", "VERSION_PROBE_UNAVAILABLE"),
        }
        details = value.get("details")
        if (type(details) is dict
                and set(details) == {"kind", "reason", "cleanup_unverified", "cleanup_status"}
                and details.get("kind") == "capture_version_probe"
                and type(details.get("reason")) is str and details["reason"] in reasons
                and type(details.get("cleanup_unverified")) is bool
                and details.get("cleanup_status") == ("unverified" if details["cleanup_unverified"] else "not_reported")):
            display, category = reasons[details["reason"]]
            if message == "Coding-agent version probe refused: " + display:
                return category
    return (None, "APP_ERROR_1", "APP_ERROR_2", "APP_ERROR_3", "APP_ERROR_4",
            "APP_ERROR_5", "APP_ERROR_6", "APP_ERROR_7", "APP_ERROR_8")[value["code"]]


class Cancelled(BaseException):
    def __init__(self, signum):
        self.signum = signum


class ChildFailure(RuntimeError):
    pass


class RuntimeOutputLimit(ChildFailure):
    """Expose bounded metadata in a traceback, never captured output content."""
    def __init__(self):
        super().__init__("runtime output limit")
        self.observation = None

    def __str__(self):
        value = self.observation
        categories = {"REPOSITORY_OWNERSHIP", "COMMAND_OWNERSHIP", "TERMINAL_SETTLEMENT",
                      "PROFILE_QUALIFICATION", "VERSION_PROBE_ADMISSION", "VERSION_PROBE_DEADLINE",
                      "VERSION_PROBE_CLOCK", "VERSION_PROBE_OWNERSHIP", "VERSION_PROBE_UNAVAILABLE"}
        categories.update("APP_ERROR_" + str(code) for code in range(1, 9))
        if (type(value) is not dict
                or set(value) != {"stream", "observed_bytes", "allowed_bytes", "error_category"}
                or type(value["stream"]) is not str or value["stream"] not in ("stdout", "stderr")
                or any(type(value[key]) is not int or not 0 <= value[key] <= 2**64 - 1
                       for key in ("observed_bytes", "allowed_bytes"))
                or (value["error_category"] is not None
                    and (type(value["error_category"]) is not str or value["error_category"] not in categories))):
            return super().__str__()
        return ("runtime output limit: stream=" + value["stream"]
                + " observed_bytes=" + str(value["observed_bytes"])
                + " allowed_bytes=" + str(value["allowed_bytes"])
                + " category=" + (value["error_category"] or "UNKNOWN"))



# Failure-only closed metadata; no child/path/payload reads or ownership actions.
_DIAGNOSTIC_FAULTS = frozenset({
    "PEER_FAILED", "RUNTIME_ENDPOINT", "RUNTIME_DEADLINE", "RUNTIME_OUTPUT_LIMIT",
    "RUNTIME_INPUT_WRITE", "RUNTIME_INPUT_CLOSE", "RUNTIME_OUTPUT_READ",
    "RUNTIME_DIRECT_WAIT", "RUNTIME_WAIT_RECEIPT", "RUNTIME_PIPE_FLAGS",
    "RUNTIME_START_OR_RECEIPT", "RUNTIME_RESULT_RECEIPT", "RUNTIME_COMMAND_FAILED",
    "RUNTIME_PARENT_OR_SIGNAL", "RUNTIME_SLOT_POLL", "RUNTIME_SLOT_COLLECTION",
    "RUNTIME_CLEANUP_DEADLINE", "RUNTIME_CANCEL_WRITE", "RUNTIME_CONTROL_CLOSE",
    "RUNTIME_OPERATION_OR_RECEIPT", "RUNTIME_EXIT_COLLECTION", "RUNTIME_OWNER_EXIT",
    "RUNTIME_SIGNAL", "OWNER_CLOCK", "EOF_WITHOUT_TERMINAL", "READ_FAILED",
    "WRITE_FAILED", "TRUNCATED_FRAME", "POST_TERMINAL_DATA", "PEER_IDENTITY",
})
_DIAGNOSTIC_MATRIX = frozenset({
    "positive", "profile-record-changed", "managed-root-recreated",
    "native-image-changed", "registration-changed", "generation-changed",
    "generation-receipt-changed", "source-replaced", "source-symlink", "source-fifo",
    "source-hardlink", "source-directory", "source-wrong-session", "path-missing",
    "path-null", "path-wrong-namespace", "path-wrong-session", "numeric-revoked",
    "metadata-paused", "wrong-owned-native-image-bounded-chain",
})
_DIAGNOSTIC_PHASES = frozenset({
    "normal-dispatch", "pre-mutation-prompt", "post-mutation-stop",
    "wrong-origin-prompt", "terminal-after-wrong-origin",
})


def _closed_diagnostic_value(value, choices):
    return value if type(value) is str and value in choices else "UNKNOWN"


def _closed_diagnostic_fault(value):
    if value is None:
        return None
    return value if type(value) is str and value in _DIAGNOSTIC_FAULTS else "OTHER"


def owned_failure_snapshot(manager):
    """Read only retained Python facts; never poll, wait, drain, clock or repr."""
    slots = manager.slots
    slot = slots[-1] if slots else {}
    endpoint = slot.get("endpoint")
    child = slot.get("child")
    status = child.returncode if child is not None else None
    status = status if type(status) is int and -(2**31) <= status < 2**31 else None
    def flag(value):
        return value if type(value) is bool else None
    def count(value):
        return value if type(value) is int and 0 <= value <= 2**64 - 1 else None
    return {
        "slot_count": min(len(slots), 4096),
        "direct_handle_retained": child is not None,
        "direct_wait_known": status is not None,
        "direct_returncode": status,
        "accepted": slot.get("accepted") is not None,
        "collected": flag(slot.get("collected")),
        "waited_published": flag(slot.get("waited_published")),
        "stdin_closed": (child.stdin is None) if child is not None else None,
        "stdout_eof": flag(slot.get("stdout_eof")),
        "stderr_eof": flag(slot.get("stderr_eof")),
        "stdout_bytes": count(slot.get("stdout_count")),
        "stderr_bytes": count(slot.get("stderr_count")),
        "endpoint_live": flag(getattr(endpoint, "live", None)),
        "endpoint_terminal": flag(getattr(endpoint, "terminal", None)),
        "endpoint_eof": flag(getattr(endpoint, "eof", None)),
        "endpoint_settled": flag(getattr(endpoint, "settled", None)),
        "slot_fault": _closed_diagnostic_fault(slot.get("first_fault")),
        "endpoint_fault": _closed_diagnostic_fault(getattr(endpoint, "first_fault", None)),
        "owner_fault": _closed_diagnostic_fault(manager.owner.first_fault),
    }


def diagnosed_owned_operation(manager, operation, context, emit):
    """Preserve existing context cleanup and primary exception; emit only after it."""
    first = None
    try:
        with manager:
            try:
                return operation(manager)
            except BaseException:
                try:
                    first = owned_failure_snapshot(manager)
                except BaseException:
                    pass
                raise
    except BaseException:
        try:
            record = {
                "schema": 1, "source_fixture_only": True,
                "namespace": _closed_diagnostic_value(context.get("namespace"), {"codex", "claude-code"}),
                "matrix_label": _closed_diagnostic_value(context.get("matrix_label"), _DIAGNOSTIC_MATRIX),
                "phase": _closed_diagnostic_value(context.get("phase"), _DIAGNOSTIC_PHASES),
                "event": _closed_diagnostic_value(context.get("event"), {"prompt", "stop"}),
                "first": first, "after_cleanup": owned_failure_snapshot(manager),
            }
            emit(record)
        except BaseException:
            pass  # Diagnostics never replace the original exception or retry work.
        raise


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
        self.hard_end = None

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
        self.owner.fail_manager(self, reason)

    def _now(self):
        at = self.owner.now()
        if self.owner.deadline_ns == 0:
            from source_qa_protocol import ProtocolError
            self.hard_end = self.cleanup_end = 0
            self._fail(ProtocolError("OWNER_CLOCK"), "OWNER_CLOCK")
            # begin_cleanup performs no clock read: a recovered later clock
            # cannot reopen zero or replace the original operation exception.
            self.begin_cleanup(0)
        return at

    def _restore_handlers(self):
        for signum, handler in self.handlers.items():
            try:
                signal.signal(signum, handler)
            except BaseException as error:
                self._fail(error, "RUNTIME_SIGNAL_RESTORE")

    def _exit_owned_owner(self, exception):
        if not self.owns_owner:
            return
        import source_qa_protocol as protocol
        try:
            self.owner.__exit__(type(exception) if exception is not None else None,
                                exception, exception.__traceback__ if exception is not None else None)
        finally:
            protocol.CURRENT = self.owner.previous

    def _cancel(self, signum, _frame):
        self.pending_cancel = signum
        if not self.spawning:
            self.owner.fail("RUNTIME_SIGNAL")

    def __enter__(self):
        if self.entered:
            raise ChildFailure("runtime manager reused")
        self.entered = True
        try:
            if self.owns_owner:
                self.owner.__enter__()
            self.owner.register_manager(self)
            for signum in (signal.SIGTERM, signal.SIGINT):
                self.handlers[signum] = signal.signal(signum, self._cancel)
        except BaseException as error:
            self._fail(error, "RUNTIME_ENTER")
            try:
                self._restore_handlers()
            finally:
                try: self._exit_owned_owner(error)
                except BaseException as secondary: self._fail(secondary, "RUNTIME_OWNER_EXIT")
            raise
        return self

    def __exit__(self, kind, exception, traceback):
        if exception is not None:
            self._fail(exception, "RUNTIME_OPERATION_OR_RECEIPT")
        try:
            self.begin_cleanup(self.hard_end if self.hard_end is not None else self._now())
            while not self.poll_cleanup(self.cleanup_end):
                if self._now() >= self.cleanup_end:
                    self.owner.fail("RUNTIME_CLEANUP_DEADLINE")
                    break
                time.sleep(0.001)
        except BaseException as error:
            self._fail(error, "RUNTIME_EXIT_COLLECTION")
        finally:
            try:
                self._restore_handlers()
            finally:
                try: self._exit_owned_owner(exception if exception is not None else self.first_error)
                except BaseException as error: self._fail(error, "RUNTIME_OWNER_EXIT")
        if exception is None and self.first_error is not None:
            raise self.first_error
        return False

    def _select_end(self, timeout, installer=False):
        if installer and (self.owner.managers != [self] or self.slots
                          or self.hard_end is not None or self.owner.runtime_end_ns is not None):
            raise ChildFailure("installer must be the first runtime admission")
        self.owner.check()
        at = self._now()
        maximum = INSTALL_OPERATION_NS if installer else 4_000_000_000
        self.owner.limit_end(at + maximum)
        end = min(at + int(timeout * 1_000_000_000), self.owner.deadline_ns, at + maximum)
        if self.hard_end is not None:
            end = min(end, self.hard_end)
        if end <= at:
            raise ChildFailure("expired cooperative operation")
        self.hard_end = end
        if self.owner.runtime_end_ns is None:
            # Observe the actual owner admission; a shorter slot has its own end.
            # This field never grants authority or changes cleanup tightening.
            self.owner._runtime_end_ns = self.owner.deadline_ns
        return at, end

    def start_installer(self, label, arguments, extra=None, capture_limit=0, input_bytes=b""):
        case = Path(self.root)
        prepared = case.parent
        expected = [str(prepared / "natives/codex-inert-source-only"),
                    "--installer-phases-diagnostic", str(prepared), str(case),
                    str(prepared / "pair/keito")]
        if (not case.is_absolute() or Path(self.cwd) != case or arguments != expected):
            raise ChildFailure("invalid closed installer route")
        return self._start(label, arguments, INSTALL_OPERATION_NS / 1_000_000_000,
                           extra, capture_limit, input_bytes, installer=True)

    def start(self, label, arguments, timeout, extra=None, capture_limit=0, input_bytes=b""):
        return self._start(label, arguments, timeout, extra, capture_limit, input_bytes)

    def _start(self, label, arguments, timeout, extra=None, capture_limit=0, input_bytes=b"", installer=False):
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
        at, end = self._select_end(timeout, installer)
        charge = len(input_bytes) + capture_limit
        if self.owner.runtime_io_reserved + charge > 4 * 1024 * 1024:
            raise ChildFailure("runtime buffer budget")
        nonce = secrets.token_hex(16)
        parent, child_end = socket.socketpair()
        endpoint = self.owner.endpoint(parent, nonce)
        slot = {"label": label, "nonce": nonce, "child": None, "endpoint": endpoint,
                "end": end, "started": at, "input": input_bytes, "offset": 0,
                "capture_limit": capture_limit, "stdout": bytearray(), "stdout_count": 0,
                "stderr_count": 0, "stdout_eof": False, "stderr_eof": False,
                "cancel_sent": False, "waited_published": False, "collected": False,
                "accepted": None, "retrieved": False, "first_fault": None,
                "output_limit_first": None}
        slot.update({"stdin_configured": False, "stdout_configured": False, "stderr_configured": False,
                     "control_closed": False, "cancel_end": None})
        # Reserve history and buffers before Popen. Slot remains even on error.
        self.slots.append(slot)
        self.io_reserved += charge
        self.owner.runtime_io_reserved += charge
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
                signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
                os.dup2(source_fd, 4, inheritable=True)
                if source_fd != 4:
                    os.close(source_fd)
            self.spawning = True
            previous_mask = signal.pthread_sigmask(signal.SIG_BLOCK, (signal.SIGINT, signal.SIGTERM))
            try:
                self.owner.check()
                gate_at = self._now()
                self.owner.check()
                if self.pending_cancel is not None or gate_at >= end:
                    raise Cancelled(self.pending_cancel or signal.SIGTERM)
                child = subprocess.Popen(arguments, cwd=self.cwd, env=env,
                                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                         close_fds=True, pass_fds=tuple(sorted({4, source_fd})), preexec_fn=attach)
                slot["child"] = child  # BEFORE flags, endpoint identity or receipt.
            finally:
                self.spawning = False
                signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
            endpoint.expected_pid = child.pid
            if self.pending_cancel is not None:
                raise Cancelled(self.pending_cancel)
            self._configure_pipes(slot)
            self.owner.check()
            self.owner.forward([f"REGISTER|{os.getpid()}|{child.pid}|{nonce}|0"])
            # Ordinary exclusive receipt, after actual returned-handle retention.
            self._record(label + "-child-started.json", {"pid": child.pid, "launch_nonce": nonce,
                         "launch_before_monotonic_ns": at, "launch_after_monotonic_ns": self._now()})
            return slot
        except BaseException as error:
            self._fail(error, "RUNTIME_START_OR_RECEIPT")
            self.begin_cleanup(min(end, self._now() + 4_000_000_000))
            raise
        finally:
            child_end.close()
            if placeholder is not None:
                os.close(placeholder)

    def _configure_pipes(self, slot):
        for name in ("stdin", "stdout", "stderr"):
            try:
                os.set_blocking(getattr(slot["child"], name).fileno(), False)
                slot[name + "_configured"] = True
            except BaseException as error:
                self._fail(error, "RUNTIME_PIPE_FLAGS")
                slot["first_fault"] = slot["first_fault"] or "RUNTIME_PIPE_FLAGS"
        if slot["first_fault"]:
            raise self.first_error

    def _close_input(self, slot):
        child = slot["child"]
        if child is not None and child.stdin is not None:
            try:
                child.stdin.close()
                child.stdin = None
            except BaseException as error:
                slot["first_fault"] = slot["first_fault"] or "RUNTIME_INPUT_CLOSE"
                self._fail(error, "RUNTIME_INPUT_CLOSE")

    def _close_control(self, slot):
        if not slot.get("control_closed", False):
            try:
                slot["endpoint"].close()
                slot["control_closed"] = True
            except BaseException as error:
                self._fail(error, "RUNTIME_CONTROL_CLOSE")

    def _poll_slot(self, slot, accepting):
        from source_qa_protocol import ProtocolError
        child = slot["child"]
        if child is None:
            self._close_control(slot)
            slot["collected"] = slot.get("control_closed", False)
            return
        endpoint = slot["endpoint"]
        try:
            if endpoint.first_fault:
                self._fail(ProtocolError(endpoint.first_fault), "RUNTIME_ENDPOINT")
                endpoint.drain_failed()
            else:
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
                self._close_input(slot)
            elif slot.get("stdin_configured", False):
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
                    self._close_input(slot)
        for name in ("stdout", "stderr"):
            pipe = getattr(child, name)
            if pipe is None or not slot.get(name + "_configured", False):
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
                    error = RuntimeOutputLimit()
                    self._fail(error, "RUNTIME_OUTPUT_LIMIT")
                    if slot.get("output_limit_first") is None:
                        observation = {"stream": name, "observed_bytes": slot[name + "_count"],
                                       "allowed_bytes": slot["capture_limit"] if name == "stdout" else 0,
                                       "error_category": None}
                        slot["output_limit_first"] = observation
                        # The original failure is already latched. Optional
                        # classification cannot replace it or retain raw stderr.
                        if name == "stderr":
                            try: observation["error_category"] = _stderr_category(block)
                            except BaseException: pass
                            else: error.observation = observation.copy()
                        else:
                            error.observation = observation.copy()
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
                             and child.stdin is None and endpoint.eof)
        at = self._now()
        if (accepting and slot["accepted"] is None and slot["collected"] and endpoint.settled
                and slot["first_fault"] is None and self.owner.first_fault is None
                and at < slot["end"]):
            slot["accepted"] = at
        if slot["collected"]:
            self._close_control(slot)
            if not slot.get("control_closed", False): slot["collected"] = False

    def poll(self):
        try:
            self.owner.check()
        except BaseException as error:
            self._fail(error, "RUNTIME_PARENT_OR_SIGNAL")
        if self.owner.first_fault:
            self.begin_cleanup(self.hard_end if self.hard_end is not None else self._now())
        for slot in self.slots:
            if not slot["collected"]:
                try: self._poll_slot(slot, self.owner.first_fault is None)
                except BaseException as error: self._fail(error, "RUNTIME_SLOT_POLL")
                if self.owner.first_fault:
                    self.begin_cleanup(self.hard_end if self.hard_end is not None else slot["end"])
            if slot["accepted"] is None and self._now() >= slot["end"]:
                self._fail(subprocess.TimeoutExpired(slot["label"], 0), "RUNTIME_DEADLINE")
                self.begin_cleanup(self.hard_end if self.hard_end is not None else slot["end"])
        if self.owner.first_fault:
            self.begin_cleanup(self.hard_end if self.hard_end is not None else self._now())
        return all(slot["accepted"] is not None for slot in self.slots)

    def begin_cleanup(self, end):
        bounds = [end]
        if self.cleanup_end is not None: bounds.append(self.cleanup_end)
        if self.hard_end is not None: bounds.append(self.hard_end)
        bounds.extend(slot["end"] for slot in self.slots if slot["accepted"] is None)
        if self.owner.deadline_ns is not None:
            bounds.append(self.owner.deadline_ns)
        self.cleanup_end = min(bounds)
        # Broadcast ALL cancellation/close intents before any wait or drain.
        for slot in self.slots:
            if slot["child"] is None:
                # Close the provisional parent channel even at a zero end.
                # The surrounding start finally owns the temporary peer/mask;
                # only its later collection turn may mark the slot collected.
                self._close_control(slot)
                continue
            if slot["accepted"] is not None:
                continue
            child = slot["child"]
            self._close_input(slot)
            if (not slot["endpoint"].settled and (slot.get("cancel_end") is None or self.cleanup_end < slot["cancel_end"])):
                slot["cancel_sent"] = True
                slot["cancel_end"] = self.cleanup_end
                try:
                    slot["endpoint"].cancel_tighter(max(1, self.cleanup_end))
                except BaseException as error:
                    self._fail(error, "RUNTIME_CANCEL_WRITE")

    def poll_cleanup(self, end):
        self.owner.pump_parent()
        self.begin_cleanup(end)
        for slot in self.slots:
            if not slot["collected"]:
                if self._now() >= self.cleanup_end:
                    self._fail(ChildFailure("runtime cleanup deadline"), "RUNTIME_CLEANUP_DEADLINE")
                    self.begin_cleanup(self.cleanup_end)
                try: self._poll_slot(slot, False)
                except BaseException as error: self._fail(error, "RUNTIME_SLOT_COLLECTION")
                if self._now() >= self.cleanup_end:
                    self._fail(ChildFailure("runtime cleanup deadline"), "RUNTIME_CLEANUP_DEADLINE")
                    self.begin_cleanup(self.cleanup_end)
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
            self.begin_cleanup(self._now() + 4_000_000_000)
            raise
        if child.returncode != 0:
            error = ChildFailure("cooperative runtime child failed")
            self._fail(error, "RUNTIME_COMMAND_FAILED")
            raise error
        return bytes(slot["stdout"]) if slot["capture_limit"] else None

    def run(self, label, arguments, timeout, extra=None, capture_limit=0, input_bytes=b""):
        return self.complete(self.start(label, arguments, timeout, extra, capture_limit, input_bytes))
