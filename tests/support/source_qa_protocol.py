"""Developer-only cancellation/settlement metadata; no capture/auth authority.

Not yet connected to process-owning QA runners. A clean protocol trace alone
never proves reserved-group absence or an actual direct wait.
"""
import os
import signal
import socket
import time

FD = "KEITO_SOURCE_OWNER_FD"
NONCE = "KEITO_SOURCE_OWNER_NONCE"
CURRENT = None
MAX_FRAME = 256  # Including newline.
MAX_PENDING = 1024
MAX_READS = 8  # Attempts, including interrupted reads.
MAX_PARSE = 32
MAX_REGISTRATIONS = 4096
MAX_FRAMES = 16384
MAX_WIRE = 4 * 1024 * 1024


class ProtocolError(RuntimeError):
    """Only bounded reason codes, never offending input."""


def nonce_valid(value):
    return isinstance(value, str) and len(value) == 32 and all(c in "0123456789abcdef" for c in value)


def integer(value, low, high):
    digits = value[1:] if value.startswith("-") else value
    if not value or len(value) > 21 or not digits.isascii() or not digits.isdigit():
        raise ProtocolError("INTEGER_SCHEMA")
    result = int(value)
    if str(result) != value:
        raise ProtocolError("INTEGER_SCHEMA")
    if not low <= result <= high:
        raise ProtocolError("INTEGER_RANGE")
    return result


class Budget:
    """Shared send/receive/forwarding and retained-trace budget per operation."""
    def __init__(self):
        self.wire = self.frames = self.registrations = 0
        self.first_fault = None
        self.owner_fault = None

    def poison_owner(self, reason):
        if self.owner_fault is None:
            self.owner_fault = reason

    def charge(self, kind, amount, limit):
        if self.first_fault:
            raise ProtocolError(self.first_fault)
        if getattr(self, kind) + amount > limit:
            self.first_fault = "LIMIT_" + kind.upper()
            raise ProtocolError(self.first_fault)
        setattr(self, kind, getattr(self, kind) + amount)


class MetadataLedger:
    """The same bounded grammar/ledger at an endpoint and at an outer root."""
    def __init__(self, root_pid, budget):
        self.root_pid, self.budget = root_pid, budget
        self.records = {}

    def observe(self, value):
        if (not isinstance(value, str) or len(value) + 34 > MAX_FRAME
                or not value.isascii() or "\n" in value or "\r" in value):
            raise ProtocolError("METADATA_SCHEMA")
        fields = value.split("|")
        if len(fields) != 5 or fields[0] not in ("REGISTER", "WAITED") or not nonce_valid(fields[3]):
            raise ProtocolError("METADATA_SCHEMA")
        owner = integer(fields[1], 1, 2147483647)
        child = integer(fields[2], 1, 2147483647)
        identity = (owner, child, fields[3])
        if fields[0] == "REGISTER":
            integer(fields[4], 0, 18446744073709551615)
            if owner != self.root_pid and not any(key[1] == owner and not waited for key, waited in self.records.items()):
                raise ProtocolError("OWNER_EDGE")
            if (owner == child or child == self.root_pid or identity in self.records
                    or any(key[1] == child and not waited for key, waited in self.records.items())):
                raise ProtocolError("DUPLICATE_REGISTRATION")
            self.budget.charge("registrations", 1, MAX_REGISTRATIONS)
            self.records[identity] = False
        else:
            integer(fields[4], -2147483648, 2147483647)
            if identity not in self.records or self.records[identity]:
                raise ProtocolError("UNMATCHED_WAIT")
            self.records[identity] = True

    def complete(self):
        return all(self.records.values())


class Endpoint:
    def __init__(self, channel, nonce, expected_pid=None, budget=None, on_fault=None):
        def reject(reason):
            if on_fault is not None:
                on_fault(reason)
            raise ProtocolError(reason)
        if not nonce_valid(nonce):
            reject("NONCE_SCHEMA")
        if channel.family != socket.AF_UNIX or channel.type != socket.SOCK_STREAM:
            reject("SOCKET_SCHEMA")
        if expected_pid is not None and (type(expected_pid) is not int or not 1 <= expected_pid <= 2147483647):
            reject("PID_SCHEMA")
        self.channel, self.nonce, self.expected_pid = channel, nonce, expected_pid
        try:
            channel.setblocking(False)
            channel.set_inheritable(False)
        except OSError:
            reject("SOCKET_FLAGS")
        self.budget = budget if budget is not None else Budget()
        self.on_fault = on_fault
        self.pending = bytearray()
        self.live = self.settled = self.cancelled = False
        self.terminal = self.eof = False
        self.first_fault = None
        self.cancel_deadline_ns = None
        self.events = []
        self.registrations = {}
        self._parent_role = None
        self._sent_live = self._sent_terminal = self._sent_cancel = False
        self._failed_attempted = False
        self._sent_ledger = MetadataLedger(os.getpid(), self.budget)
        self._sent_registrations = self._sent_ledger.records

    def fail(self, reason):
        if self.first_fault is None:
            self.first_fault = reason
        self.settled = False
        self.budget.poison_owner(self.first_fault)
        if self.on_fault is not None:
            self.on_fault(self.first_fault)
        raise ProtocolError(self.first_fault)

    def _check(self):
        if self.first_fault:
            raise ProtocolError(self.first_fault)
        if self.budget.first_fault:
            self.fail(self.budget.first_fault)

    def _send(self, value):
        if not isinstance(value, str) or len(value) + 34 > MAX_FRAME or "\n" in value or "\r" in value:
            self.fail("FRAME_SCHEMA")
        try:
            frame = (self.nonce + "|" + value + "\n").encode("ascii")
            self.budget.charge("wire", len(frame), MAX_WIRE)
            self.budget.charge("frames", 1, MAX_FRAMES)
            if self.channel.send(frame) != len(frame):
                self.fail("SHORT_WRITE")
        except (OSError, UnicodeError):
            self.fail("WRITE_FAILED")
        except ProtocolError as error:
            self.fail(str(error))

    def send(self, tag, deadline_ns=None):
        # One bounded FAILED attempt is allowed after a fault. It never repairs
        # proof; a broken/exhausted channel instead leaves EOF without settlement.
        if tag == "FAILED":
            if self._failed_attempted or self._sent_terminal:
                self.fail("DUPLICATE_TERMINAL")
            self._failed_attempted = self._sent_terminal = True
            self._send("FAILED|" + str(os.getpid()))
            try:
                self.channel.shutdown(socket.SHUT_WR)
            except OSError:
                self.fail("SHUTDOWN_FAILED")
            return
        self._check()
        if tag == "SETTLED" and self.budget.owner_fault is not None:
            self.fail(self.budget.owner_fault)
        if self._sent_terminal:
            self.fail("POST_TERMINAL_WRITE")
        if tag == "LIVE" and not self._sent_live and not self._sent_cancel:
            self._sent_live = True
        elif tag == "SETTLED" and self._sent_live and all(self._sent_registrations.values()):
            self._sent_terminal = True
        elif tag == "CANCEL" and not self._sent_cancel and not self._sent_live:
            self._sent_cancel = True
            if type(deadline_ns) is not int or not 1 <= deadline_ns <= 18446744073709551615:
                self.fail("CANCEL_DEADLINE")
            self._send("CANCEL|{}|{}".format(os.getpid(), deadline_ns))
            return
        else:
            self.fail("SEND_STATE")
        self._send(tag + "|" + str(os.getpid()))
        if self._sent_terminal:
            try:
                self.channel.shutdown(socket.SHUT_WR)
            except OSError:
                self.fail("SHUTDOWN_FAILED")

    def metadata(self, value):
        self._check()
        if not self._sent_live or self._sent_terminal:
            self.fail("METADATA_SEND_STATE")
        try:
            self._sent_ledger.observe(value)
            self._send(value)
        except ProtocolError as error:
            self.fail(str(error))

    def _frame(self, line, parent):
        if self.terminal:
            self.fail("POST_TERMINAL_DATA")
        if len(line) + 1 > MAX_FRAME:
            self.fail("FRAME_LIMIT")
        self.budget.charge("frames", 1, MAX_FRAMES)
        try:
            fields = line.decode("ascii").split("|")
        except UnicodeError:
            self.fail("FRAME_ENCODING")
        if len(fields) not in (3, 4, 6) or fields[0] != self.nonce:
            self.fail("NONCE_OR_SCHEMA")
        tag = fields[1]
        if len(fields) == 6:
            if parent or not self.live or tag not in ("REGISTER", "WAITED") or not nonce_valid(fields[4]):
                self.fail("METADATA_STATE")
            owner = integer(fields[2], 1, 2147483647)
            child = integer(fields[3], 1, 2147483647)
            identity = (owner, child, fields[4])
            if tag == "REGISTER":
                if owner != self.expected_pid and not any(key[1] == owner and not waited for key, waited in self.registrations.items()):
                    self.fail("OWNER_EDGE")
                integer(fields[5], 0, 18446744073709551615)
                if (owner == child or child == self.expected_pid or identity in self.registrations
                        or any(key[1] == child and not waited for key, waited in self.registrations.items())):
                    self.fail("DUPLICATE_REGISTRATION")
                self.budget.charge("registrations", 1, MAX_REGISTRATIONS)
                self.registrations[identity] = False
            else:
                integer(fields[5], -2147483648, 2147483647)
                if identity not in self.registrations or self.registrations[identity]:
                    self.fail("UNMATCHED_WAIT")
                self.registrations[identity] = True
            self.events.append("|".join(fields[1:]))
            return
        pid = integer(fields[2], 1, 2147483647)
        if self.expected_pid is not None and pid != self.expected_pid:
            self.fail("PEER_IDENTITY")
        if parent:
            if len(fields) != 4 or tag != "CANCEL":
                self.fail("PARENT_STATE")
            deadline = integer(fields[3], 1, 18446744073709551615)
            self.cancel_deadline_ns = min(self.cancel_deadline_ns or deadline, deadline)
            self.cancelled = True
            self.expected_pid = pid
        elif len(fields) != 3:
            self.fail("CHILD_SCHEMA")
        elif tag == "LIVE" and not self.live:
            self.live, self.expected_pid = True, pid
        elif tag == "SETTLED" and self.live and all(self.registrations.values()):
            self.terminal = True
        elif tag == "FAILED":
            self.fail("PEER_FAILED")
        else:
            self.fail("CHILD_STATE")

    def receive(self, parent=False):
        self._check()
        if self._parent_role is None:
            self._parent_role = parent
        if self._parent_role != parent:
            self.fail("ROLE_CHANGED")
        if self.eof:
            return self.cancelled
        reads = parsed = 0
        try:
            while True:
                while b"\n" in self.pending and parsed < MAX_PARSE:
                    end = self.pending.index(10)
                    line = bytes(self.pending[:end])
                    del self.pending[:end + 1]
                    self._frame(line, parent)
                    parsed += 1
                if self.terminal and self.pending:
                    self.fail("POST_TERMINAL_DATA")
                if b"\n" not in self.pending and len(self.pending) >= MAX_FRAME:
                    self.fail("FRAME_LIMIT")
                if parsed == MAX_PARSE or reads == MAX_READS:
                    break
                reads += 1
                try:
                    block = self.channel.recv(MAX_FRAME)
                except BlockingIOError:
                    break
                except InterruptedError:
                    continue
                if not block:
                    self.eof = True
                    if self.pending:
                        self.fail("TRUNCATED_FRAME")
                    if parent:
                        self.cancelled = True
                    elif not self.terminal:
                        self.fail("EOF_WITHOUT_TERMINAL")
                    else:
                        self.settled = True
                    break
                self.budget.charge("wire", len(block), MAX_WIRE)
                if len(self.pending) + len(block) > MAX_PENDING:
                    self.fail("PENDING_LIMIT")
                self.pending.extend(block)
        except OSError:
            self.fail("READ_FAILED")
        except ProtocolError as error:
            self.fail(str(error))
        return self.cancelled

    def close(self):
        self.channel.close()


class SourceOwner:
    """Permanent owner fault/admission latch; not yet integrated into runners."""
    def __init__(self):
        self.parent = None
        self.managers = []
        self.cancelled = self.unsettled = self.admission_closed = False
        self.first_fault = None
        self.previous = None
        self.deadline_ns = None
        self.budget = Budget()
        self._forward_ledger = MetadataLedger(os.getpid(), self.budget)
        self._finished_managers = set()
        self._closed = False

    def fail(self, reason):
        if self.first_fault is None:
            self.first_fault = reason
        self.budget.poison_owner(self.first_fault)
        self.unsettled = self.admission_closed = True

    def __enter__(self):
        global CURRENT
        self.previous = CURRENT
        descriptor, nonce = os.environ.pop(FD, None), os.environ.pop(NONCE, None)
        try:
            if (descriptor is None) != (nonce is None):
                raise ProtocolError("PARTIAL_ENVIRONMENT")
            if descriptor is not None:
                fd = integer(descriptor, 4, 1024)
                self.parent = Endpoint(socket.socket(fileno=fd), nonce, budget=self.budget, on_fault=self.fail)
                self.parent.send("LIVE")
        except (OSError, ValueError, ProtocolError):
            self.fail("OWNER_ADMISSION")
            if self.parent:
                self.parent.close()
            raise ProtocolError(self.first_fault)
        CURRENT = self
        return self

    def check(self):
        if self.first_fault:
            raise ProtocolError(self.first_fault)
        if self.budget.first_fault or self.budget.owner_fault:
            self.fail(self.budget.first_fault or self.budget.owner_fault)
            raise ProtocolError(self.first_fault)
        if self.admission_closed and not self.cancelled:
            self.fail("OWNER_ADMISSION_CLOSED")
            raise ProtocolError(self.first_fault)
        try:
            if self.parent:
                self.cancelled |= self.parent.receive(parent=True)
                if self.parent.cancel_deadline_ns is not None:
                    received = self.parent.cancel_deadline_ns
                    self.deadline_ns = min(self.deadline_ns or received, received)
        except ProtocolError as error:
            self.fail(str(error))
            raise
        if self.cancelled:
            self.admission_closed = True
            from source_qa_children import Cancelled
            raise Cancelled(signal.SIGTERM)

    def forward(self, events):
        try:
            if self.first_fault:
                raise ProtocolError(self.first_fault)
            if self._closed:
                raise ProtocolError("OWNER_ALREADY_FINISHED")
            for count, event in enumerate(events):
                if count >= MAX_FRAMES or not isinstance(event, str) or len(event) + 34 > MAX_FRAME:
                    raise ProtocolError("FORWARD_LIMIT")
                self.budget.charge("wire", len(event) + 34, MAX_WIRE)
                self.budget.charge("frames", 1, MAX_FRAMES)
                self._forward_ledger.observe(event)
                if self.parent:
                    self.parent.metadata(event)
        except ProtocolError as error:
            self.fail(str(error))
            raise

    def endpoint(self, channel, nonce, expected_pid=None):
        self.check()
        try:
            return Endpoint(channel, nonce, expected_pid, budget=self.budget, on_fault=self.fail)
        except (OSError, ProtocolError) as error:
            self.fail(str(error) if isinstance(error, ProtocolError) else "ENDPOINT_ADMISSION")
            raise ProtocolError(self.first_fault)

    def register_manager(self, manager):
        self.check()
        if len(self.managers) >= MAX_REGISTRATIONS or any(value is manager for value in self.managers):
            self.fail("MANAGER_LIMIT_OR_DUPLICATE")
            raise ProtocolError(self.first_fault)
        self.managers.append(manager)

    def finish_managers(self):
        self.admission_closed = True
        if self.budget.first_fault or self.budget.owner_fault:
            self.fail(self.budget.first_fault or self.budget.owner_fault)
        deadline = time.monotonic_ns() + 4_000_000_000
        self.deadline_ns = min(self.deadline_ns or deadline, deadline)
        if len(self.managers) > MAX_REGISTRATIONS:
            self.fail("MANAGER_LIMIT")
            raise ProtocolError(self.first_fault)
        # Broadcast before waiting. active==None or direct wait is not proof.
        pending = []
        for manager in self.managers:
            if id(manager) in self._finished_managers:
                continue
            try:
                manager.begin_cleanup(self.deadline_ns)
                pending.append(manager)
            except BaseException:
                self.fail("MANAGER_CLEANUP_BEGIN")
        while pending:
            if time.monotonic_ns() >= self.deadline_ns:
                self.fail("MANAGER_CLEANUP_DEADLINE")
                break
            remaining = []
            for manager in pending:
                try:
                    if time.monotonic_ns() >= self.deadline_ns:
                        self.fail("MANAGER_CLEANUP_DEADLINE")
                        remaining.append(manager)
                        continue
                    result = manager.poll_cleanup(self.deadline_ns)
                    if time.monotonic_ns() >= self.deadline_ns:
                        self.fail("MANAGER_CLEANUP_DEADLINE")
                        remaining.append(manager)
                    elif result is False:
                        remaining.append(manager)
                    elif result is True:
                        self._finished_managers.add(id(manager))
                    else:
                        self.fail("MANAGER_PROOF_SCHEMA")
                except BaseException:
                    self.fail("MANAGER_CLEANUP_POLL")
            pending = remaining
            if pending and time.monotonic_ns() >= self.deadline_ns:
                self.fail("MANAGER_CLEANUP_DEADLINE")
                break
            if pending:
                time.sleep(0.001)
        if not self._forward_ledger.complete():
            self.fail("UNWAITED_FORWARD_REGISTRATION")

    def __exit__(self, exception_type, exception, traceback):
        global CURRENT
        terminal_error = None
        if isinstance(exception, ProtocolError):
            self.fail(str(exception))
        try:
            self.finish_managers()
            if self.parent:
                try:
                    self.parent.send("FAILED" if self.unsettled else "SETTLED")
                except (ProtocolError, OSError) as error:
                    self.fail("TERMINAL_WRITE")
                    terminal_error = error
        finally:
            self._closed = True
            if self.parent:
                self.parent.close()
            CURRENT = self.previous
        if exception is None and (self.first_fault or terminal_error):
            raise ProtocolError(self.first_fault or "TERMINAL_WRITE")
