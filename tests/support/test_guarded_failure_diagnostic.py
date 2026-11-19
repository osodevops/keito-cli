"""Pure diagnostic fixtures. No subprocess, socket, file, clock or runtime roots."""
import json
from pathlib import Path
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch
sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from source_qa_children import diagnosed_owned_operation, owned_failure_snapshot


class FakeManager:
    def __init__(self):
        self.owner = SimpleNamespace(first_fault="RUNTIME_ENDPOINT")
        self.child = SimpleNamespace(returncode=None, stdin=object())
        self.endpoint = SimpleNamespace(first_fault="PEER_FAILED", live=True,
                                        terminal=False, eof=False, settled=False)
        self.slots = [{"child": self.child, "endpoint": self.endpoint, "accepted": None,
                       "collected": False, "waited_published": False, "stdout_eof": False,
                       "stderr_eof": False, "stdout_count": 0, "stderr_count": 0,
                       "first_fault": "PEER_FAILED", "stdout": b"PRIVATE_PAYLOAD"}]
        self.cleanup_calls = 0
    def __enter__(self):
        return self
    def __exit__(self, kind, error, traceback):
        self.cleanup_calls += 1
        self.child.returncode = 1
        self.child.stdin = None
        self.endpoint.eof = True
        self.slots[0].update(collected=True, stdout_eof=True, stderr_eof=True)
        return False


CONTEXT = {"namespace": "codex", "matrix_label": "profile-record-changed",
           "phase": "pre-mutation-prompt", "event": "prompt"}


class FailureDiagnosticTests(unittest.TestCase):
    def test_primary_identity_cleanup_then_distinct_first_and_final(self):
        manager, rows = FakeManager(), []
        primary = RuntimeError("PRIVATE_ERROR")
        def operation(active):
            self.assertIs(active, manager)
            raise primary
        def emit(value):
            self.assertEqual(manager.cleanup_calls, 1)
            rows.append(value)
        with self.assertRaises(RuntimeError) as caught:
            diagnosed_owned_operation(manager, operation, CONTEXT, emit)
        self.assertIs(caught.exception, primary)
        self.assertEqual(manager.cleanup_calls, 1)
        self.assertEqual(len(rows), 1)
        self.assertFalse(rows[0]["first"]["direct_wait_known"])
        self.assertEqual(rows[0]["after_cleanup"]["direct_returncode"], 1)
        self.assertFalse(rows[0]["first"]["stdin_closed"])
        self.assertTrue(rows[0]["after_cleanup"]["stdin_closed"])
        self.assertFalse(rows[0]["first"]["endpoint_eof"])
        self.assertTrue(rows[0]["after_cleanup"]["endpoint_eof"])
        self.assertFalse(rows[0]["after_cleanup"]["endpoint_settled"])
        self.assertEqual(rows[0]["first"]["endpoint_fault"], "PEER_FAILED")
        self.assertEqual(rows[0]["phase"], "pre-mutation-prompt")

    def test_success_is_silent_and_cleans_once(self):
        manager, rows = FakeManager(), []
        self.assertEqual(diagnosed_owned_operation(manager, lambda _: 42, CONTEXT, rows.append), 42)
        self.assertEqual(rows, [])
        self.assertEqual(manager.cleanup_calls, 1)

    def test_emitter_failure_does_not_replace_primary(self):
        manager = FakeManager()
        primary = RuntimeError("PRIVATE_PRIMARY")
        def fail(_):
            raise primary
        def emit(_):
            raise ValueError("PRIVATE_DIAGNOSTIC_FAILURE")
        with self.assertRaises(RuntimeError) as caught:
            diagnosed_owned_operation(manager, fail, CONTEXT, emit)
        self.assertIs(caught.exception, primary)
        self.assertEqual(manager.cleanup_calls, 1)

    def test_snapshot_failure_does_not_delay_cleanup_or_replace_primary(self):
        manager = FakeManager()
        primary = RuntimeError("PRIVATE_PRIMARY")
        def fail(_):
            raise primary
        with patch("source_qa_children.owned_failure_snapshot", side_effect=ValueError("PRIVATE")):
            with self.assertRaises(RuntimeError) as caught:
                diagnosed_owned_operation(manager, fail, CONTEXT, lambda _: None)
        self.assertIs(caught.exception, primary)
        self.assertEqual(manager.cleanup_calls, 1)

    def test_closed_payload_ignores_free_context_errors_paths_and_raw_output(self):
        manager, rows = FakeManager(), []
        manager.endpoint.first_fault = "PRIVATE_ENDPOINT_ERROR"
        manager.owner.first_fault = {"PRIVATE_PATH": "/PRIVATE_SECRET"}
        manager.slots[0]["first_fault"] = "PRIVATE_SLOT_ERROR"
        context = {key: "PRIVATE_CONTEXT" for key in CONTEXT}
        def fail(_):
            raise RuntimeError("PRIVATE_ERROR")
        with self.assertRaises(RuntimeError):
            diagnosed_owned_operation(manager, fail, context, rows.append)
        self.assertEqual(len(rows), 1)
        raw = json.dumps(rows[0])
        self.assertNotIn("PRIVATE", raw)
        self.assertLess(len(raw.encode()), 4096)
        self.assertEqual(rows[0]["namespace"], "UNKNOWN")
        self.assertEqual(rows[0]["after_cleanup"]["owner_fault"], "OTHER")
        self.assertEqual(set(rows[0]), {"schema", "source_fixture_only", "namespace", "matrix_label",
                                      "phase", "event", "first", "after_cleanup"})

    def test_cached_unknown_signal_and_bad_numeric_types_without_poll(self):
        manager = FakeManager()
        manager.child.returncode = -15
        manager.slots[0]["stdout_count"] = True
        manager.slots[0]["stderr_count"] = 2**64
        value = owned_failure_snapshot(manager)
        self.assertEqual(value["direct_returncode"], -15)
        self.assertTrue(value["direct_wait_known"])
        self.assertIsNone(value["stdout_bytes"])
        self.assertIsNone(value["stderr_bytes"])
        manager.child.returncode = True
        self.assertFalse(owned_failure_snapshot(manager)["direct_wait_known"])

    def test_empty_provisional_slot_is_unknown_without_handle(self):
        manager = FakeManager()
        manager.slots = []
        value = owned_failure_snapshot(manager)
        self.assertEqual(value["slot_count"], 0)
        self.assertFalse(value["direct_handle_retained"])
        self.assertFalse(value["direct_wait_known"])
        self.assertIsNone(value["direct_returncode"])


if __name__ == "__main__":
    unittest.main()
