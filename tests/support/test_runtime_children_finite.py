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
from source_qa_children import OwnedChildren, Cancelled, ChildFailure
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
        with patch("source_qa_protocol.monotonic_ns", return_value=100):
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

    def test_second_review_unspawned_failed_close_remains_unknown(self):
        self.a["child"] = None
        def cannot_close(): raise OSError("inert-control-close")
        self.a["endpoint"].close = cannot_close
        self.manager._poll_slot(self.a, False)
        self.assertFalse(self.a["control_closed"])
        self.assertFalse(self.a["collected"])

    def test_second_review_expiry_after_a_poll_broadcasts_before_b_progress(self):
        clock = [100]
        self.a["end"] = 200
        self.b["end"] = 1000
        def progress(value, accepting):
            self.trace.append((value["label"], "ordinary-poll"))
            if value is self.a: clock[0] = 200
        self.manager._poll_slot = progress
        with patch("source_qa_protocol.monotonic_ns", side_effect=lambda: clock[0]): self.manager.poll()
        b_poll = self.trace.index(("b", "ordinary-poll"))
        self.assertTrue(all(any(event[:2] == (name, "CANCEL") for event in self.trace[:b_poll]) for name in ["a", "b"]))

    def test_second_review_receipt_clock_failure_preserves_original_and_zeroes_siblings(self):
        self.a["accepted"] = 5
        self.a["collected"] = True
        self.a["child"].returncode = 0
        (self.root / "a-result.json").write_text("retained")
        with patch("source_qa_protocol.monotonic_ns", side_effect=OSError("inert-clock")):
            try:
                self.manager.complete(self.a)
            except BaseException as error:
                observed = error
        self.assertIsInstance(observed, FileExistsError)
        self.assertIs(self.manager.first_error, observed)
        self.assertEqual(self.owner.deadline_ns, 0)
        self.assertEqual(self.manager.cleanup_end, 0)
        self.assertTrue(self.b["cancel_sent"])

    def test_second_review_failed_rollback_still_exits_owner_and_restores_current(self):
        import source_qa_protocol as protocol
        previous = protocol.CURRENT
        manager = OwnedChildren(self.root, {}, self.root)
        original = OSError("inert-install")
        calls = []
        def install(signum, handler):
            calls.append(signum)
            if len(calls) == 2: raise original
            if len(calls) == 3: raise OSError("inert-restore")
            return object()
        with patch("signal.signal", side_effect=install):
            try: manager.__enter__()
            except BaseException as error: observed = error
        self.assertIs(observed, original)
        self.assertIs(protocol.CURRENT, previous)
        self.assertTrue(manager.owner._closed)

    def test_second_review_late_cleanup_never_becomes_clean_manager_proof(self):
        self.owner.managers = [self.manager]
        self.owner.deadline_ns = 4000
        self.a["end"] = self.b["end"] = 900
        for value in [self.a, self.b]:
            value["endpoint"].settled = value["endpoint"].eof = True
            value["child"].returncode = 0
        clock = [800]
        original_poll = self.a["child"].poll
        def cross_bound(): clock[0] = 901; return original_poll()
        self.a["child"].poll = cross_bound
        with patch("source_qa_protocol.monotonic_ns", side_effect=lambda: clock[0]), patch("os.read", return_value=b""):
            self.owner.finish_managers()
        self.assertTrue(self.a["collected"] and self.b["collected"])
        self.assertNotIn(id(self.manager), self.owner._finished_managers)
        self.assertTrue(self.owner.unsettled)

    def test_second_review_admission_gate_clock_failure_never_calls_popen(self):
        owner = SourceOwner()
        manager = OwnedChildren(self.root, {}, self.root, owner)
        manager.entered = True
        armed = [False]
        def mask(*args): armed[0] = True; return set()
        def now():
            if armed[0]:
                armed[0] = False
                raise OSError("inert-gate-clock")
            return 100
        # First gate check succeeds. The failure is the following fresh read.
        original_check = owner.check
        def gate_check():
            if armed[0]:
                armed[0] = False
                original_check()
                armed[0] = True
            else: original_check()
        with patch("signal.pthread_sigmask", side_effect=mask), patch("source_qa_protocol.monotonic_ns", side_effect=now), patch.object(owner, "check", side_effect=gate_check), patch("subprocess.Popen", side_effect=RuntimeError("POpen must not execute")) as popen:
            with self.assertRaises(BaseException): manager.start("clock-gate", ["never-execute"], 1)
        popen.assert_not_called()
        self.assertEqual(owner.deadline_ns, 0)
        self.assertEqual(manager.cleanup_end, 0)
        self.assertTrue(manager.slots[0]["control_closed"])

    def test_review_cached_owner_cannot_finish_clean_at_deadline(self):
        owner = SourceOwner()
        owner.deadline_ns = 100
        with patch("source_qa_protocol.monotonic_ns", return_value=100):
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


class InstallerPolicyTests(unittest.TestCase):
    def manager(self, parent=None):
        owner = SourceOwner()
        owner.deadline_ns = parent
        manager = OwnedChildren(Path("/private/synthetic/case"), {},
                                Path("/private/synthetic/case"), owner)
        owner.register_manager(manager)
        manager.entered = True
        return manager

    @patch("source_qa_protocol.monotonic_ns", return_value=100_000_000)
    def test_installer_original_end_and_cleanup_observation_are_distinct(self, _clock):
        manager = self.manager()
        self.assertEqual(manager._select_end(30, installer=True), (100_000_000, 30_100_000_000))
        self.assertEqual(manager.owner.runtime_end_ns, 30_100_000_000)
        manager.owner.limit_end(4_100_000_000)
        self.assertEqual(manager.owner.deadline_ns, 4_100_000_000)
        self.assertEqual(manager.owner.runtime_end_ns, 30_100_000_000)
        with self.assertRaises(AttributeError):
            manager.owner.runtime_end_ns = 90_000_000_000
        with self.assertRaises(ChildFailure):
            manager._select_end(30, installer=True)

    @patch("source_qa_protocol.monotonic_ns", return_value=100_000_000)
    def test_installer_preserves_earlier_parent_and_cannot_follow_other_runtime(self, _clock):
        manager = self.manager(4_000_000_000)
        self.assertEqual(manager._select_end(30, installer=True)[1], 4_000_000_000)
        ordinary = self.manager()
        self.assertEqual(ordinary._select_end(30)[1], 4_100_000_000)
        with self.assertRaises(ChildFailure):
            ordinary._select_end(30, installer=True)
        sibling = OwnedChildren(ordinary.root, {}, ordinary.cwd, ordinary.owner)
        ordinary.owner.register_manager(sibling)
        with self.assertRaises(ChildFailure):
            sibling._select_end(30, installer=True)

    @patch("source_qa_protocol.monotonic_ns", return_value=100_000_000)
    def test_ordinary_and_hook_limits_do_not_change(self, _clock):
        manager = self.manager()
        self.assertEqual(manager._select_end(1)[1], 1_100_000_000)
        with patch("source_qa_protocol.monotonic_ns", return_value=200_000_000):
            self.assertEqual(manager._select_end(600)[1], 1_100_000_000)
        self.assertEqual(manager.owner.runtime_end_ns, 4_100_000_000)

    @patch("source_qa_protocol.monotonic_ns", return_value=100_000_000)
    def test_short_slot_retains_actual_owner_admission_and_separate_cleanup(self, _clock):
        for parent, original, slot_end in [
                (None, 4_100_000_000, 1_100_000_000),
                (3_000_000_000, 3_000_000_000, 1_100_000_000),
                (800_000_000, 800_000_000, 800_000_000)]:
            with self.subTest(parent=parent):
                manager = self.manager(parent)
                self.assertEqual(manager._select_end(1)[1], slot_end)
                self.assertEqual(manager.hard_end, slot_end)
                self.assertEqual(manager.owner.deadline_ns, original)
                self.assertEqual(manager.owner.runtime_end_ns, original)
                manager.owner.limit_end(700_000_000)
                self.assertEqual(manager.owner.deadline_ns, 700_000_000)
                self.assertEqual(manager.owner.runtime_end_ns, original)
                self.assertEqual(manager._select_end(30)[1], 700_000_000)
                self.assertEqual(manager.owner.runtime_end_ns, original)

    @patch("source_qa_protocol.monotonic_ns", return_value=100_000_000)
    def test_installer_route_is_closed_before_any_runtime_preparation(self, _clock):
        manager = self.manager()
        prepared = manager.root.parent
        argv = [str(prepared / "natives/codex-inert-source-only"),
                "--installer-phases-diagnostic", str(prepared), str(manager.root),
                str(prepared / "pair/keito")]
        with patch.object(manager, "_start", return_value="selected") as start:
            self.assertEqual(manager.start_installer("install", argv), "selected")
            self.assertEqual(start.call_args.args[2], 30)
            self.assertTrue(start.call_args.kwargs["installer"])
            for bad in [argv[:-1], argv + ["extra"], ["/usr/bin/git"] + argv[1:],
                        [argv[0], "--cli"] + argv[2:], argv[:-1] + ["relative"]]:
                with self.assertRaises(ChildFailure):
                    manager.start_installer("install", bad)
            self.assertEqual(start.call_count, 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
