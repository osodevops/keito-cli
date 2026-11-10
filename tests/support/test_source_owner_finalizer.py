"""Finite R34 manager-finalizer regressions: objects/fake clocks only.

No processes, profiles, signals, threads, runtime actors or environment access.
The real SourceOwner is the subject; test managers have no cleanup authority.
"""
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import source_qa_protocol as protocol
from source_qa_protocol import SourceOwner, ProtocolError
from source_qa_children import Cancelled


class Parent:
    def __init__(self, close_error=False):
        self.sent = []
        self.closed = False
        self.close_error = close_error

    def send(self, tag):
        self.sent.append(tag)

    def close(self):
        self.closed = True
        if self.close_error:
            raise OSError("PRIVATE_CLOSE_DETAIL")


class FinalizerCases(unittest.TestCase):
    def test_begin_error_retains_manager_and_broadcasts_siblings_before_poll(self):
        owner = SourceOwner()
        calls = []
        class Manager:
            def __init__(self, name): self.name, self.polls = name, 0
            def begin_cleanup(self, end):
                calls.append(("begin", self.name, end))
                if self.name == "a": raise OSError("PRIVATE_BEGIN_DETAIL")
            def poll_cleanup(self, end):
                self.polls += 1
                calls.append(("poll", self.name, end))
                return self.name == "b" or self.polls == 2
        a, b = Manager("a"), Manager("b")
        owner.managers = [a, b]
        with patch.object(protocol.time, "monotonic_ns", return_value=1), patch.object(protocol.time, "sleep"):
            owner.finish_managers()
        self.assertEqual([(kind, name) for kind, name, _ in calls],
                         [("begin", "a"), ("begin", "b"), ("poll", "a"), ("poll", "b"), ("poll", "a")])
        self.assertEqual(len({end for _, _, end in calls}), 1)
        self.assertEqual(owner.managers, [a, b])
        self.assertEqual(owner.first_fault, "MANAGER_CLEANUP_BEGIN")
        self.assertTrue(owner.unsettled)
        self.assertNotIn(id(a), owner._finished_managers)

    def test_poll_error_retains_manager_for_later_collection(self):
        owner = SourceOwner()
        class Manager:
            polls = 0
            def begin_cleanup(self, end): pass
            def poll_cleanup(self, end):
                self.polls += 1
                if self.polls == 1: raise OSError("PRIVATE_POLL_DETAIL")
                return True
        manager = Manager()
        owner.managers = [manager]
        with patch.object(protocol.time, "monotonic_ns", return_value=1), patch.object(protocol.time, "sleep"):
            owner.finish_managers()
        self.assertEqual(manager.polls, 2)
        self.assertEqual(owner.first_fault, "MANAGER_CLEANUP_POLL")
        self.assertNotIn(id(manager), owner._finished_managers)
        before = manager.polls
        with patch.object(protocol.time, "monotonic_ns", return_value=2):
            owner.finish_managers()
        self.assertEqual(manager.polls, before)  # collected failed facts are cached, never clean proof
        self.assertTrue(owner.unsettled)

    def test_bad_schema_does_not_discard_pending_collection(self):
        owner = SourceOwner()
        class Manager:
            polls = 0
            def begin_cleanup(self, end): pass
            def poll_cleanup(self, end):
                self.polls += 1
                return "NOT_A_PROOF" if self.polls == 1 else True
        manager = Manager()
        owner.managers = [manager]
        with patch.object(protocol.time, "monotonic_ns", return_value=1), patch.object(protocol.time, "sleep"):
            owner.finish_managers()
        self.assertEqual(manager.polls, 2)
        self.assertEqual(owner.first_fault, "MANAGER_PROOF_SCHEMA")
        self.assertNotIn(id(manager), owner._finished_managers)

    def test_generic_receipt_error_and_cancellation_cannot_emit_settled(self):
        for original in (OSError("PRIVATE_RECEIPT_DETAIL"), Cancelled(15), KeyboardInterrupt()):
            with self.subTest(kind=type(original).__name__):
                owner = SourceOwner()
                parent = Parent()
                owner.parent = parent
                with patch.object(protocol.time, "monotonic_ns", return_value=1):
                    self.assertFalse(owner.__exit__(type(original), original, None))
                self.assertEqual(parent.sent, ["FAILED"])
                self.assertTrue(parent.closed)
                self.assertTrue(owner.admission_closed)
                self.assertTrue(owner.unsettled)
                self.assertNotIn("PRIVATE", owner.first_fault)

    def test_finish_exception_does_not_replace_original_or_skip_failed_terminal(self):
        owner = SourceOwner()
        parent = Parent()
        owner.parent = parent
        original = OSError("PRIVATE_ORIGINAL")
        with patch.object(owner, "finish_managers", side_effect=RuntimeError("PRIVATE_CLEANUP")):
            self.assertFalse(owner.__exit__(OSError, original, None))
        self.assertEqual(parent.sent, ["FAILED"])
        self.assertTrue(parent.closed)
        self.assertEqual(owner.first_fault, "OWNER_OPERATION_FAILED")

    def test_close_exception_preserves_original_and_restores_current(self):
        owner = SourceOwner()
        owner.parent = Parent(close_error=True)
        previous = object()
        owner.previous = previous
        original = ProtocolError("ORIGINAL")
        with patch.object(protocol, "CURRENT", owner), patch.object(protocol.time, "monotonic_ns", return_value=1):
            self.assertFalse(owner.__exit__(ProtocolError, original, None))
            self.assertIs(protocol.CURRENT, previous)
        self.assertEqual(owner.first_fault, "ORIGINAL")
        self.assertTrue(owner._closed)

    def test_permanent_callback_error_remains_unfinished_at_original_bound(self):
        owner = SourceOwner()
        owner.deadline_ns = 10
        class Manager:
            polls = 0
            def begin_cleanup(self, end): pass
            def poll_cleanup(self, end):
                self.polls += 1
                raise OSError("PRIVATE_CALLBACK")
        manager = Manager()
        owner.managers = [manager]
        ticks = iter(range(1, 100))
        with patch.object(protocol.time, "monotonic_ns", side_effect=lambda: next(ticks)), patch.object(protocol.time, "sleep"):
            owner.finish_managers()
        self.assertGreater(manager.polls, 1)
        self.assertEqual(owner.deadline_ns, 10)
        self.assertEqual(owner.first_fault, "MANAGER_CLEANUP_POLL")
        self.assertNotIn(id(manager), owner._finished_managers)
        self.assertEqual(owner.managers, [manager])


if __name__ == "__main__":
    unittest.main()
