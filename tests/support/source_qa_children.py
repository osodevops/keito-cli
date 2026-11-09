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


class OwnedChildren:
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
                OwnedChildren._record(self, label + "-cleanup-fallback.json", result)
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
