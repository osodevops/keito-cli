"""Finite tests of the REAL runtime manager, no Popen/fork/tool execution."""
import os
from pathlib import Path
import sys
import signal
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from source_qa_children import OwnedChildren, Cancelled
from source_qa_protocol import SourceOwner, ProtocolError


class Pipe:
    def __init__(self, fd): self.fd, self.closed = fd, False
    def fileno(self): return self.fd
    def close(self): self.closed = True


class Child:
    def __init__(self, pid, status=None):
        self.pid, self.returncode = pid, status
        self.stdin, self.stdout, self.stderr = Pipe(100), Pipe(101), Pipe(102)
    def poll(self): return self.returncode


class Peer:
    def __init__(self, name, events):
        self.name, self.trace = name, events
        self.settled = self.eof = False
        self.first_fault = None
        self.events = []
        self.closed = False
    def send(self, tag, deadline_ns=None): self.trace.append((self.name, tag, deadline_ns))
    def receive(self, parent=False):
        self.trace.append((self.name, "poll"))
        if self.first_fault: raise ProtocolError(self.first_fault)
    def close(self): self.closed = True
    def cancel_tighter(self, end): self.send("CANCEL", deadline_ns=end)
    def drain_failed(self): return self.eof


def slot(name, pid, trace, end):
    return {"label": name, "nonce": name * 32, "child": Child(pid), "endpoint": Peer(name, trace),
            "end": end, "started": 1, "input": b"", "offset": 0,
            "capture_limit": 8, "stdout": bytearray(), "stdout_count": 0, "stderr_count": 0,
            "stdout_eof": False, "stderr_eof": False, "cancel_sent": False,
            "waited_published": False, "collected": False, "accepted": None,
            "retrieved": False, "first_fault": None}


class RuntimeManagerTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="finite-manager-", dir=os.environ["TMPDIR"]))
        self.owner = SourceOwner()
        self.manager = OwnedChildren(self.root, {}, self.root, self.owner)
        self.trace = []
        self.end = 10**30
        self.a = slot("a", 101, self.trace, self.end)
        self.b = slot("b", 102, self.trace, self.end)
        self.manager.slots = [self.a, self.b]
        for value in self.manager.slots:
            value.update({"stdin_configured": True, "stdout_configured": True, "stderr_configured": True,
                          "control_closed": False, "cancel_end": None})
        self.owner.forward([f"REGISTER|{os.getpid()}|101|{'a'*32}|0",
                            f"REGISTER|{os.getpid()}|102|{'b'*32}|0"])

    def test_review_failed_endpoint_is_not_physical_eof(self):
        self.a["endpoint"].first_fault = "INERT_ENDPOINT"
        self.a["child"].returncode = 0
        with patch("os.read", return_value=b""):
            self.manager._poll_slot(self.a, False)
        self.assertTrue(self.a["stdout_eof"] and self.a["stderr_eof"])
        self.assertEqual(self.a["child"].returncode, 0)
        self.assertFalse(self.a["endpoint"].eof)
        self.assertFalse(self.a["collected"])

    def test_review_known_fault_broadcasts_before_any_sibling_poll(self):
        self.owner.fail("ORIGINAL")
        def progress(slot, accepting): self.trace.append((slot["label"], "ordinary-poll"))
        self.manager._poll_slot = progress
        self.manager.poll()
        cancel_indices = [i for i, event in enumerate(self.trace) if event[1] == "CANCEL"]
        poll_indices = [i for i, event in enumerate(self.trace) if event[1] == "ordinary-poll"]
        self.assertEqual(len(cancel_indices), 2)
        self.assertLess(max(cancel_indices), min(poll_indices))

    def test_review_close_error_cannot_skip_sibling_cancel(self):
        def fail_close(): raise OSError("inert-close")
        self.a["child"].stdin.close = fail_close
        try:
            self.manager.begin_cleanup(self.end)
        except OSError:
            pass
        self.assertTrue(self.b["cancel_sent"])

    def test_review_cancel_never_extends_original_slot_and_tightens(self):
        self.a["end"] = 1000
        self.b["end"] = 900
        with patch("time.monotonic_ns", return_value=100):
            self.manager.begin_cleanup(5000)
            self.manager.begin_cleanup(500)
            self.manager.begin_cleanup(6000)
        a_ends = [event[2] for event in self.trace if event[:2] == ("a", "CANCEL")]
        b_ends = [event[2] for event in self.trace if event[:2] == ("b", "CANCEL")]
        self.assertEqual(a_ends, [900, 500])
        self.assertEqual(b_ends, [900, 500])

    def test_review_unconfigured_output_is_never_read(self):
        self.a["stdout_configured"] = False
        self.a["stderr_configured"] = True
        with patch("os.read", return_value=b"") as read:
            self.manager._poll_slot(self.a, False)
        self.assertNotIn(101, [call.args[0] for call in read.call_args_list])
        self.assertFalse(self.a["stdout_eof"])

    def test_review_each_stream_flags_attempted_after_an_independent_failure(self):
        for failed in [100, 101, 102]:
            owner = SourceOwner()
            manager = OwnedChildren(self.root, {}, self.root, owner)
            value = slot("a", 101, [], self.end)
            value.update({"stdin_configured": False, "stdout_configured": False, "stderr_configured": False})
            attempted = []
            def configure(fd, blocking):
                attempted.append(fd)
                if fd == failed: raise OSError("inert-flags")
            with patch("os.set_blocking", side_effect=configure), self.assertRaises(OSError):
                manager._configure_pipes(value)
            self.assertEqual(attempted, [100, 101, 102])
            for name, fd in [("stdin",100),("stdout",101),("stderr",102)]:
                self.assertEqual(value[name + "_configured"], fd != failed)

    def test_review_latched_cancel_during_preparation_never_calls_popen(self):
        owner = SourceOwner()
        manager = OwnedChildren(self.root, {}, self.root, owner)
        manager.entered = True
        def during_preparation(fd):
            manager._cancel(signal.SIGTERM, None)
            return object()
        with patch("os.fstat", side_effect=during_preparation), patch("signal.pthread_sigmask", return_value=set()), patch("subprocess.Popen") as popen:
            with self.assertRaises((ProtocolError, Cancelled)):
                manager.start("cancelled", ["never-execute"], 1)
        popen.assert_not_called()
        self.assertEqual(owner.first_fault, "RUNTIME_SIGNAL")
        self.assertEqual(len(manager.slots), 1)
        self.assertIsNone(manager.slots[0]["child"])
        manager.poll_cleanup(manager.hard_end)

    def test_review_failed_enter_restores_current_and_partial_handlers(self):
        import source_qa_protocol as protocol
        previous = protocol.CURRENT
        manager = OwnedChildren(self.root, {}, self.root)
        calls = []
        old = object()
        def install(signum, handler):
            calls.append((signum, handler))
            if len(calls) == 2: raise OSError("inert-signal-setup")
            return old
        with patch("signal.signal", side_effect=install), self.assertRaises(OSError):
            manager.__enter__()
        self.assertIs(protocol.CURRENT, previous)
        self.assertEqual(calls[-1], (signal.SIGTERM, old))
        self.assertEqual(len(calls), 3)

    def test_review_cached_owner_cannot_finish_clean_at_deadline(self):
        owner = SourceOwner()
        owner.deadline_ns = 100
        with patch("time.monotonic_ns", return_value=100):
            owner.finish_managers()
        self.assertIsNotNone(owner.first_fault)

    def test_review_completed_control_is_explicitly_closed(self):
        self.a["endpoint"].settled = self.a["endpoint"].eof = True
        self.a["child"].returncode = 0
        with patch("os.read", return_value=b""):
            self.manager._poll_slot(self.a, True)
        self.assertTrue(self.a["endpoint"].closed)
        self.assertTrue(self.a["collected"])

    def test_review_poll_exception_does_not_starve_sibling_collection(self):
        def progress(slot, accepting):
            self.trace.append((slot["label"], "poll"))
            if slot is self.a: raise OSError("inert-poll")
        self.manager._poll_slot = progress
        try:
            self.manager.poll_cleanup(self.end)
        except OSError:
            pass
        self.assertIn(("b", "poll"), self.trace)

    def test_broadcast_precedes_all_polls_and_sibling_cancel_survives_error(self):
        def fail(*args, **kwargs):
            self.trace.append(("a", "cancel-failed")); raise OSError("inert")
        self.a["endpoint"].send = fail
        self.manager.begin_cleanup(self.end)
        self.assertEqual([x[0] for x in self.trace], ["a", "b"])
        self.assertTrue(self.a["cancel_sent"] and self.b["cancel_sent"])
        self.assertTrue(self.a["child"].stdin is None and self.b["child"].stdin is None)
        self.assertEqual(self.owner.first_fault, "RUNTIME_CANCEL_WRITE")
        original = self.manager.cleanup_end
        self.manager.begin_cleanup(self.end * 2)
        self.assertLessEqual(self.manager.cleanup_end, original)
        self.assertEqual(len(self.trace), 2)

    def test_endpoint_failure_still_collects_direct_wait_and_both_real_eofs(self):
        self.a["endpoint"].first_fault = "INERT_ENDPOINT"
        self.a["child"].returncode = 0
        with patch("os.read", return_value=b""):
            self.manager._poll_slot(self.a, False)
        # Contract correction AO-04: wait/pipe EOF are retained, but a failed
        # endpoint is no longer substituted for physical control EOF.
        self.assertFalse(self.a["collected"])
        self.assertTrue(self.a["stdout_eof"] and self.a["stderr_eof"])
        self.assertIsNone(self.a["accepted"])
        self.assertEqual(self.owner.first_fault, "RUNTIME_ENDPOINT")

    def test_successful_a_completion_leaves_b_live_and_history_retained(self):
        self.a["endpoint"].settled = self.a["endpoint"].eof = True
        self.a["child"].returncode = 0
        with patch("os.read", return_value=b""):
            self.manager._poll_slot(self.a, True)
        self.assertIsNotNone(self.a["accepted"])
        self.assertIsNone(self.b["child"].returncode)
        self.assertFalse(self.b["collected"])
        self.assertIs(self.manager.active, self.b)
        self.assertEqual(len(self.manager.slots), 2)
        self.assertFalse(self.owner._forward_ledger.complete())

    def test_actual_result_eexist_cancels_live_sibling_and_preserves_exception(self):
        self.a["accepted"] = 5
        self.a["collected"] = True
        self.a["child"].returncode = 0
        self.a["endpoint"].settled = self.a["endpoint"].eof = True
        (self.root / "a-result.json").write_text("retain collision")
        with self.assertRaises(FileExistsError) as raised:
            self.manager.complete(self.a)
        self.assertIs(self.manager.first_error, raised.exception)
        self.assertEqual(self.owner.first_fault, "RUNTIME_RESULT_RECEIPT")
        self.assertTrue(self.b["cancel_sent"])
        self.assertIsNone(self.b["child"].returncode)
        self.assertEqual((self.root / "a-result.json").read_text(), "retain collision")
        self.assertEqual(len(self.manager.slots), 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
