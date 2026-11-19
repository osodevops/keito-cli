"""R17 independently derived finite cases, promoted unchanged to regression CI.

Only Unix socketpairs and metadata: no child actors, signals, threads, environment
or profile inspection. These assertions express the requested component contract.
The original reviewer source and first RED are retained in developer evidence.
"""
import os
import socket
import sys
import unittest

from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
from source_qa_protocol import Endpoint, SourceOwner, ProtocolError, MAX_REGISTRATIONS

NONCE = "a" * 32
PEER = 12345


class IndependentlyDerivedCases(unittest.TestCase):
    def sockets(self):
        left, right = socket.socketpair()
        self.addCleanup(left.close)
        self.addCleanup(right.close)
        return left, right

    def test_owner_fault_blocks_other_endpoint_clean_terminal(self):
        owner = SourceOwner()
        up, upstream_peer = self.sockets()
        upstream = owner.endpoint(up, NONCE)
        upstream.send("LIVE")
        down, downstream_peer = self.sockets()
        downstream = owner.endpoint(down, NONCE, PEER)
        downstream_peer.sendall((("b" * 32) + "|LIVE|12345\n").encode("ascii"))
        with self.assertRaises(ProtocolError) as first:
            downstream.receive()
        self.assertEqual(owner.first_fault, str(first.exception))
        self.assertTrue(owner.unsettled)
        # At b2ac4ae2 this does not raise; it writes SETTLED and shuts down.
        with self.assertRaises(ProtocolError):
            upstream.send("SETTLED")

    def test_root_forward_refuses_unmatched_wait(self):
        owner = SourceOwner()
        event = "WAITED|{}|12346|{}|0".format(os.getpid(), "b" * 32)
        with self.assertRaises(ProtocolError):
            owner.forward([event])
        self.assertTrue(owner.unsettled)
        self.assertTrue(owner.admission_closed)

    def test_root_forward_registration_cap_is_operation_wide(self):
        owner = SourceOwner()
        own_pid = os.getpid()
        # 8194 frames and less than 1 MiB: only the registration cap is crossed.
        with self.assertRaises(ProtocolError) as fault:
            for index in range(MAX_REGISTRATIONS + 1):
                child = 100000 + index
                if child == own_pid:
                    child += 50000
                suffix = "{}|{}|{}".format(own_pid, child, "b" * 32)
                owner.forward(["REGISTER|" + suffix + "|1"])
                owner.forward(["WAITED|" + suffix + "|0"])
        self.assertEqual(str(fault.exception), "LIMIT_REGISTRATIONS")
        self.assertTrue(owner.unsettled)

    def test_clean_terminal_every_byte_split_requires_eof(self):
        terminal = (NONCE + "|SETTLED|12345\n").encode("ascii")
        for split in range(1, len(terminal)):
            with self.subTest(split=split):
                left, right = self.sockets()
                endpoint = Endpoint(left, NONCE, PEER)
                right.sendall((NONCE + "|LIVE|12345\n").encode("ascii"))
                endpoint.receive()
                right.sendall(terminal[:split])
                endpoint.receive()
                self.assertFalse(endpoint.settled)
                right.sendall(terminal[split:])
                endpoint.receive()
                self.assertFalse(endpoint.settled)
                right.shutdown(socket.SHUT_WR)
                endpoint.receive()
                self.assertTrue(endpoint.settled)
                left.close()
                right.close()


if __name__ == "__main__":
    unittest.main()
