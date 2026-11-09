#!/usr/bin/env python3
"""Finite socket-only ownership regressions. No subprocesses or signals."""
from pathlib import Path
import os
import json
import socket
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from source_qa_protocol import (Endpoint, ProtocolError, SourceOwner, Budget,
                                MAX_FRAME, MAX_PENDING, MAX_READS, MAX_PARSE,
                                MAX_REGISTRATIONS, MAX_FRAMES, MAX_WIRE)

NONCE = "a" * 32
PEER = 12345


class ProtocolTests(unittest.TestCase):
    def pair(self):
        reader, writer = socket.socketpair()
        self.addCleanup(reader.close)
        self.addCleanup(writer.close)
        return Endpoint(reader, NONCE, PEER), writer

    def frame(self, writer, tag, *fields, nonce=NONCE):
        values = fields or (PEER,)
        writer.sendall(("|".join((nonce, tag, *(str(v) for v in values))) + "\n").encode())

    def live(self, endpoint, writer):
        self.frame(writer, "LIVE")
        endpoint.receive()

    def test_clean_terminal_requires_closed_writer_control(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        self.frame(writer, "REGISTER", PEER, 12346, "b" * 32, 1)
        self.frame(writer, "WAITED", PEER, 12346, "b" * 32, 0)
        self.frame(writer, "SETTLED")
        writer.shutdown(socket.SHUT_WR)
        endpoint.receive()
        self.assertTrue(endpoint.settled)

    def test_bad_nonce_cannot_be_rehabilitated_by_later_settlement(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        self.frame(writer, "LIVE", nonce="b" * 32)
        with self.assertRaises(ProtocolError) as first:
            endpoint.receive()
        self.frame(writer, "SETTLED")
        writer.shutdown(socket.SHUT_WR)
        with self.assertRaises(ProtocolError) as later:
            endpoint.receive()
        self.assertEqual(str(first.exception), str(later.exception))
        self.assertFalse(endpoint.settled)

    def test_unmatched_wait_cannot_be_rehabilitated(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        self.frame(writer, "WAITED", PEER, 12346, "b" * 32, 0)
        with self.assertRaises(ProtocolError) as first:
            endpoint.receive()
        self.frame(writer, "SETTLED")
        writer.shutdown(socket.SHUT_WR)
        with self.assertRaises(ProtocolError) as later:
            endpoint.receive()
        self.assertEqual(str(first.exception), str(later.exception))
        self.assertFalse(endpoint.settled)

    def test_terminal_without_eof_is_not_settled(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        self.frame(writer, "SETTLED")
        endpoint.receive()
        self.assertFalse(endpoint.settled)
        writer.shutdown(socket.SHUT_WR)
        endpoint.receive()
        self.assertTrue(endpoint.settled)

    def test_partial_trailing_frame_poisons_terminal(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        self.frame(writer, "SETTLED")
        endpoint.receive()
        writer.sendall(b"trailing")
        writer.shutdown(socket.SHUT_WR)
        with self.assertRaises(ProtocolError):
            endpoint.receive()
        self.assertFalse(endpoint.settled)

    def test_registration_before_live_is_refused(self):
        endpoint, writer = self.pair()
        self.frame(writer, "REGISTER", PEER, 12346, "b" * 32, 1)
        with self.assertRaises(ProtocolError):
            endpoint.receive()

    def test_unregistered_owner_edge_is_refused(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        self.frame(writer, "REGISTER", 9999, 12346, "b" * 32, 1)
        with self.assertRaises(ProtocolError):
            endpoint.receive()

    def test_truncated_eof_is_refused(self):
        endpoint, writer = self.pair()
        writer.sendall(b"partial")
        writer.shutdown(socket.SHUT_WR)
        with self.assertRaises(ProtocolError):
            endpoint.receive()

    def test_oversized_complete_frame_is_refused(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        self.frame(writer, "REGISTER", PEER, 12346, "b" * 32, "9" * 300)
        with self.assertRaises(ProtocolError):
            endpoint.receive()

    def test_source_owner_retains_fault_and_emits_failed_after_cleanup(self):
        reader, writer = socket.socketpair()
        self.addCleanup(reader.close)
        self.addCleanup(writer.close)
        owner = SourceOwner()
        owner.parent = Endpoint(reader, NONCE, os.getpid())
        owner.parent.send("LIVE")
        self.frame(writer, "CANCEL", os.getpid(), nonce="b" * 32)
        with self.assertRaises(ProtocolError) as first:
            owner.check()
        owner.__exit__(ProtocolError, first.exception, None)
        writer.settimeout(1)
        trace = bytearray()
        while True:
            block = writer.recv(1024)
            if not block:
                break
            trace.extend(block)
        self.assertIn(b"|FAILED|", trace)
        self.assertNotIn(b"|SETTLED|", trace)
        self.assertTrue(owner.unsettled)

    def test_all_control_faults_remain_sticky(self):
        for tag, fields in [("LIVE", (PEER,)), ("FAILED", (PEER,)),
                            ("LIVE", (PEER + 1,)), ("unknown", (PEER,))]:
            with self.subTest(tag=tag, fields=fields):
                endpoint, writer = self.pair()
                self.live(endpoint, writer)
                self.frame(writer, tag, *fields)
                with self.assertRaises(ProtocolError) as first:
                    endpoint.receive()
                self.frame(writer, "SETTLED")
                writer.shutdown(socket.SHUT_WR)
                with self.assertRaises(ProtocolError) as later:
                    endpoint.receive()
                self.assertEqual(str(first.exception), str(later.exception))
                self.assertFalse(endpoint.settled)

    def test_post_terminal_full_frames_never_yield_proof(self):
        for trailing in ["LIVE", "SETTLED", "REGISTER", "WAITED"]:
            with self.subTest(trailing=trailing):
                endpoint, writer = self.pair()
                self.live(endpoint, writer)
                self.frame(writer, "SETTLED")
                self.frame(writer, trailing)
                writer.shutdown(socket.SHUT_WR)
                with self.assertRaises(ProtocolError):
                    endpoint.receive()
                self.assertFalse(endpoint.settled)

    def test_parent_eof_cancels_before_live(self):
        endpoint, writer = self.pair()
        writer.shutdown(socket.SHUT_WR)
        self.assertTrue(endpoint.receive(parent=True))
        self.assertTrue(endpoint.cancelled)
        self.assertFalse(endpoint.settled)

    def test_cancel_deadline_never_extends(self):
        endpoint, writer = self.pair()
        for deadline in [100, 200, 50]:
            self.frame(writer, "CANCEL", PEER, deadline)
            self.assertTrue(endpoint.receive(parent=True))
        self.assertEqual(endpoint.cancel_deadline_ns, 50)

    def test_wrong_role_and_malformed_integer_poison(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        with self.assertRaises(ProtocolError):
            endpoint.receive(parent=True)
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        self.frame(writer, "REGISTER", "--1", 12346, "b" * 32, 1)
        with self.assertRaises(ProtocolError):
            endpoint.receive()
        self.assertIsNotNone(endpoint.first_fault)

    def test_total_budgets_exact_cap_and_cap_plus_one(self):
        for kind, maximum in [("wire", MAX_WIRE), ("frames", MAX_FRAMES),
                              ("registrations", MAX_REGISTRATIONS)]:
            with self.subTest(kind=kind):
                budget = Budget()
                budget.charge(kind, maximum, maximum)
                self.assertEqual(getattr(budget, kind), maximum)
                with self.assertRaises(ProtocolError) as first:
                    budget.charge(kind, 1, maximum)
                with self.assertRaises(ProtocolError) as later:
                    budget.charge(kind, 0, maximum)
                self.assertEqual(str(first.exception), str(later.exception))

    def test_shared_budget_counts_send_receive_and_forwarding(self):
        budget = Budget()
        left, right = socket.socketpair()
        self.addCleanup(left.close)
        self.addCleanup(right.close)
        sender = Endpoint(left, NONCE, budget=budget)
        receiver = Endpoint(right, NONCE, os.getpid(), budget=budget)
        sender.send("LIVE")
        sent = budget.wire
        receiver.receive()
        self.assertEqual(budget.wire, sent * 2)
        self.assertEqual(budget.frames, 2)
        sender.metadata("REGISTER|{}|12346|{}|1".format(os.getpid(), "b" * 32))
        receiver.receive()
        self.assertEqual(budget.registrations, 2)
        self.assertEqual(budget.frames, 4)
        with self.assertRaises(ProtocolError):
            sender.send("SETTLED")

    def test_send_failure_poison_is_visible_at_owner(self):
        class ShortSocket:
            family, type = socket.AF_UNIX, socket.SOCK_STREAM
            def setblocking(self, value): pass
            def set_inheritable(self, value): pass
            def send(self, frame): return len(frame) - 1
        owner = SourceOwner()
        endpoint = Endpoint(ShortSocket(), NONCE, on_fault=owner.fail)
        with self.assertRaises(ProtocolError):
            endpoint.send("LIVE")
        self.assertTrue(owner.admission_closed)
        self.assertTrue(owner.unsettled)
        with self.assertRaises(ProtocolError) as later:
            owner.check()
        self.assertEqual(str(later.exception), "SHORT_WRITE")

    def test_poll_read_attempt_and_parse_work_are_bounded(self):
        class BurstSocket:
            family, type = socket.AF_UNIX, socket.SOCK_STREAM
            def __init__(self, interrupted=False):
                self.calls, self.interrupted = 0, interrupted
            def setblocking(self, value): pass
            def set_inheritable(self, value): pass
            def recv(self, size):
                self.calls += 1
                if self.interrupted: raise InterruptedError()
                return (NONCE + "|CANCEL|12345|100\n").encode()
        for interrupted in [False, True]:
            channel = BurstSocket(interrupted)
            endpoint = Endpoint(channel, NONCE, PEER)
            endpoint.receive(parent=True)
            self.assertEqual(channel.calls, MAX_READS)
        # A finite prebuffer controls parse work independently of read readiness.
        channel = BurstSocket()
        endpoint = Endpoint(channel, NONCE, PEER)
        frame = (NONCE + "|CANCEL|12345|100\n").encode()
        endpoint.pending.extend(frame * 20)
        endpoint.receive(parent=True)
        self.assertLessEqual(endpoint.budget.frames, MAX_PARSE)
        self.assertLessEqual(len(endpoint.pending), MAX_PENDING)
        self.assertLessEqual(channel.calls, MAX_READS)

    def test_frame_limit_includes_newline_and_errors_hide_input(self):
        endpoint, writer = self.pair()
        self.live(endpoint, writer)
        writer.sendall(b"PRIVATE_CANARY" + b"x" * MAX_FRAME)
        with self.assertRaises(ProtocolError) as error:
            endpoint.receive()
        self.assertNotIn("PRIVATE_CANARY", str(error.exception))
        self.assertEqual(str(error.exception), "FRAME_LIMIT")

    def test_owner_broadcasts_cleanup_before_any_poll(self):
        owner = SourceOwner()
        observed = []
        class Manager:
            def __init__(self, name): self.name = name
            def begin_cleanup(self, deadline): observed.append(("begin", self.name, deadline))
            def poll_cleanup(self, deadline): observed.append(("poll", self.name, deadline)); return True
        owner.managers = [Manager("a"), Manager("b")]
        owner.finish_managers()
        self.assertEqual([(kind, name) for kind, name, _ in observed],
                         [("begin", "a"), ("begin", "b"), ("poll", "a"), ("poll", "b")])
        self.assertEqual(len({deadline for _, _, deadline in observed}), 1)
        previous = owner.deadline_ns
        owner.finish_managers()
        self.assertEqual(owner.deadline_ns, previous)
        with self.assertRaises(ProtocolError):
            owner.check()

    def test_cleanup_fault_cannot_replace_original_exception(self):
        owner = SourceOwner()
        class Manager:
            def begin_cleanup(self, deadline): raise RuntimeError("SECONDARY")
        owner.managers = [Manager()]
        original = ProtocolError("ORIGINAL")
        owner.__exit__(ProtocolError, original, None)
        self.assertEqual(owner.first_fault, "ORIGINAL")
        self.assertTrue(owner.unsettled)

    def test_constructor_refusal_and_closed_manager_admission_poison_owner(self):
        owner = SourceOwner()
        reader, writer = socket.socketpair()
        self.addCleanup(reader.close)
        self.addCleanup(writer.close)
        with self.assertRaises(ProtocolError):
            owner.endpoint(reader, "invalid", PEER)
        self.assertEqual(owner.first_fault, "NONCE_SCHEMA")
        with self.assertRaises(ProtocolError):
            owner.register_manager(object())
        self.assertEqual(owner.managers, [])
        self.assertTrue(owner.admission_closed)

    def test_shared_wire_contract_vectors(self):
        corpus = json.loads((Path(__file__).resolve().parents[1] / "fixtures/source_qa_protocol_vectors.json").read_text())
        self.assertEqual(corpus["schema"], 1)
        self.assertEqual(len(corpus["cases"]), 16)
        for case in corpus["cases"]:
            with self.subTest(case=case["name"]):
                endpoint, writer = self.pair()
                fault = False
                for chunk in case["chunks"]:
                    writer.sendall(chunk.encode("ascii"))
                    try:
                        endpoint.receive(parent=case["parent"])
                    except ProtocolError:
                        fault = True
                if case["close_writer"]:
                    writer.shutdown(socket.SHUT_WR)
                try:
                    endpoint.receive(parent=case["parent"])
                except ProtocolError:
                    fault = True
                expected = case["expected"]
                self.assertEqual(fault, expected == "fault")
                self.assertEqual(endpoint.settled, expected == "clean")
                if expected == "cancel":
                    self.assertTrue(endpoint.cancelled)


if __name__ == "__main__":
    unittest.main()
