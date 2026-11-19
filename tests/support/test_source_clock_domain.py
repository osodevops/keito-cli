"""No child processes: the wire deadline must use Rust's POSIX clock domain."""
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import source_qa_protocol as protocol


class ClockDomainTests(unittest.TestCase):
    def test_wire_clock_is_posix_monotonic_not_python_process_epoch(self):
        owner = protocol.SourceOwner()
        with patch.object(protocol.time, 'monotonic_ns', return_value=7), \
                patch.object(protocol.time, 'clock_gettime_ns', return_value=100) as clock:
            self.assertEqual(owner.now(), 100)
        clock.assert_called_once_with(protocol.time.CLOCK_MONOTONIC)
        self.assertIsNone(owner.first_fault)

    def test_posix_clock_failure_still_zeros_and_broadcasts_every_manager(self):
        owner = protocol.SourceOwner()
        owner.deadline_ns = 900
        events = []

        class Manager:
            def __init__(self, label):
                self.label = label

            def begin_cleanup(self, end):
                events.append((self.label, end))

        owner.managers = [Manager('A'), Manager('B')]
        with patch.object(protocol.time, 'monotonic_ns', return_value=7), \
                patch.object(protocol.time, 'clock_gettime_ns', side_effect=OSError('inert-clock')):
            self.assertEqual(owner.now(), 0)
        self.assertEqual(owner.deadline_ns, 0)
        self.assertEqual(owner.first_fault, 'OWNER_CLOCK')
        self.assertEqual(events, [('A', 0), ('B', 0)])

    def test_valid_posix_zero_is_not_clock_failure(self):
        owner = protocol.SourceOwner()
        with patch.object(protocol.time, 'monotonic_ns', return_value=7), \
                patch.object(protocol.time, 'clock_gettime_ns', return_value=0):
            self.assertEqual(owner.now(), 0)
        self.assertIsNone(owner.first_fault)
        self.assertIsNone(owner.deadline_ns)


if __name__ == '__main__':
    unittest.main()
