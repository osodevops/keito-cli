"""Finite tests of the REAL runtime manager, no Popen/fork/tool execution."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from source_qa_children import OwnedChildren
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
    def send(self, tag, deadline_ns=None): self.trace.append((self.name, tag, deadline_ns))
    def receive(self, parent=False):
        self.trace.append((self.name, "poll"))
        if self.first_fault: raise ProtocolError(self.first_fault)


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
        self.owner.forward([f"REGISTER|{os.getpid()}|101|{'a'*32}|0",
                            f"REGISTER|{os.getpid()}|102|{'b'*32}|0"])

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
        self.assertTrue(self.a["collected"])
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
