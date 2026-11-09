"""Pure fake/socket/metadata checks ONLY; no child, group or signal actor."""
import importlib.util
from pathlib import Path
import signal
import unittest

spec = importlib.util.spec_from_file_location("build_owner", Path(__file__).with_name("source_qa_build_owner.py"))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class Stream:
    def __init__(self, actions=()):
        self.actions = list(actions)
        self.reads = 0
        self.output = bytearray()
        self.shutdowns = 0

    def setblocking(self, _):
        pass

    def recv(self, limit):
        self.reads += 1
        value = self.actions.pop(0) if self.actions else BlockingIOError()
        if isinstance(value, BaseException):
            raise value
        assert len(value) <= limit
        return value

    def send(self, data):
        self.output.extend(data)
        return len(data)

    def shutdown(self, _):
        self.shutdowns += 1


class Group:
    def __init__(self):
        self.events = []
        self.absent_value = False
        self.fail_park = False
        self.disabled = False

    def park(self):
        assert not self.disabled
        self.events.append("park")
        if self.fail_park:
            raise OSError()

    def absent(self):
        assert not self.disabled
        self.events.append("query")
        return self.absent_value

    def send(self, signum):
        assert not self.disabled
        self.events.append("term" if signum == signal.SIGTERM else "kill")

    def abandon(self):
        assert not self.disabled
        self.events.append("self-kill")
        self.disabled = True
        raise m.Refused("SELF_GROUP_UNPROVEN")

    def disable(self):
        self.events.append("disable")
        self.disabled = True


def wire(*bodies):
    return b"".join(("a" * 32 + "|" + b + "\n").encode() for b in bodies)


def pair(actions=()):
    group = Group()
    keeper = m.Keeper(group, 10 * m.CAP)
    stream = Stream(actions)
    return keeper, group, m.Channel(stream, "a" * 32)


class Tests(unittest.TestCase):
    def test_timely_park_is_deferred_until_real_quiescence(self):
        k, g, c = pair([wire("PARK")] + [InterruptedError() for _ in range(8)])
        self.assertFalse(k.turn(c, lambda: 1))
        self.assertEqual(g.events, [])
        self.assertTrue(k.intent)
        self.assertFalse(k.turn(c, lambda: 2))
        self.assertEqual(g.events, ["park"])
        self.assertEqual(c.stream.output, wire("PARKED"))

    def test_park_and_self_kill_cannot_precede_buffered_expired_minimum(self):
        for failed in [False, True]:
            k, g, c = pair([wire("PARK", "CLEAN|100")])
            g.fail_park = failed
            with self.assertRaisesRegex(m.Refused, "DEADLINE"):
                k.turn(c, lambda: 150)
            self.assertEqual(k.end, 100)
            self.assertEqual(g.events, [])
            self.assertFalse(k.gone)

    def test_partial_wouldblock_is_not_drained(self):
        k, g, c = pair([wire("PARK") + ("a" * 32 + "|CLEAN|").encode()])
        self.assertFalse(k.turn(c, lambda: 150))
        self.assertEqual(g.events, [])
        c.stream.actions.append(b"100\n")
        with self.assertRaisesRegex(m.Refused, "DEADLINE"):
            k.turn(c, lambda: 150)
        self.assertEqual(g.events, [])

    def test_rejected_line_drains_matching_minimum_before_group_work(self):
        k, g, c = pair([b"b" * 32 + b"|PARK\n" + wire("CLEAN|100")])
        with self.assertRaisesRegex(m.Refused, "DEADLINE"):
            k.turn(c, lambda: 150)
        self.assertEqual(k.first, "CHANNEL")
        self.assertEqual(k.end, 100)
        self.assertEqual(g.events, [])

    def test_fatal_transport_stops_pending_park_and_stays_failed(self):
        k, g, c = pair([wire("PARK"), OSError()])
        with self.assertRaisesRegex(m.Refused, "CHANNEL"):
            k.turn(c, lambda: 1)
        self.assertEqual(g.events, ["disable"])
        self.assertFalse(k.intent)
        for action in [lambda: k.turn(c, lambda: 2), lambda: k.step(lambda: 2), lambda: k.park(lambda: 2)]:
            with self.assertRaises(m.Refused):
                action()
        self.assertEqual(g.events, ["disable"])

    def test_expiry_has_reserved_cleanup_budget(self):
        k, g, c = pair()
        self.assertFalse(k.turn(c, lambda: k.operation_end))
        self.assertEqual(k.first, "LIFETIME")
        self.assertEqual(g.events, ["park", "query", "term"])
        saved = list(g.events)
        with self.assertRaisesRegex(m.Refused, "DEADLINE"):
            k.turn(c, lambda: k.hard)
        self.assertEqual(g.events, saved)

    def test_first_clean_bound_is_immutable_and_second_request_refused(self):
        k, g, c = pair()
        k.control("CLEAN|1000", 1)
        with self.assertRaisesRegex(m.Refused, "CLEAN_ALREADY_BOUND"):
            k.control("CLEAN|500", 2)
        self.assertEqual(k.end, 1000)
        self.assertEqual(g.events, [])

    def test_first_clean_reduction_after_expiry_does_not_restore_proof(self):
        k, g, c = pair()
        at = k.operation_end
        k.turn(c, lambda: at)
        grace = k.term_end
        saved = list(g.events)
        c.stream.actions.append(wire("CLEAN|" + str(at + 100)))
        with self.assertRaisesRegex(m.Refused, "DEADLINE"):
            k.turn(c, lambda: at + 150)
        self.assertEqual(k.first, "LIFETIME")
        self.assertEqual(k.term_end, grace)
        self.assertEqual(g.events, saved)

    def test_pre_park_eof_parking_loss_has_one_failure_only_self_kill(self):
        k, g, c = pair([b""])
        g.fail_park = True
        with self.assertRaisesRegex(m.Refused, "SELF_GROUP_UNPROVEN"):
            k.turn(c, lambda: 1)
        self.assertEqual(k.first, "OWNER_EOF")
        self.assertEqual(g.events, ["park", "self-kill"])
        self.assertFalse(k.gone)

    def test_normal_single_bound_settlement_requires_release_and_eof(self):
        k, g, c = pair([wire("PARK", "CLEAN|1000")])
        g.absent_value = True
        self.assertFalse(k.turn(c, lambda: 1))
        self.assertEqual(c.stream.output, wire("PARKED", "GONE"))
        saved = list(g.events)
        c.stream.actions.extend([wire("RELEASE"), b""])
        self.assertTrue(k.turn(c, lambda: 2))
        self.assertEqual(g.events, saved)

    def test_trailing_release_is_sticky_failure_not_success(self):
        k, g, c = pair([wire("CLEAN|1000")])
        g.absent_value = True
        k.turn(c, lambda: 1)
        c.stream.actions.extend([wire("RELEASE", "RELEASE"), b""])
        with self.assertRaises(m.Refused):
            k.turn(c, lambda: 2)
        self.assertEqual(k.first, "CONTROL")

    def test_parent_collects_both_handles_after_first_fault(self):
        state = object.__new__(m.BuildRun)
        state.first = "FIRST"
        state.secondary = []
        state.keeper, state.child_status, state.keeper_status = 42, None, None
        state.release = False
        class Child:
            def poll(self):
                raise OSError()
        state.child = Child()
        from unittest.mock import patch
        with patch.object(m.os, "waitpid", return_value=(42, 0)) as wait:
            state.collect()
        self.assertEqual(state.first, "FIRST")
        self.assertIn("CHILD_WAIT", state.secondary)
        self.assertEqual(state.keeper_status, 0)
        wait.assert_called_once_with(42, m.os.WNOHANG)

    def test_parent_half_close_is_attempted_once_without_repeated_fault(self):
        state = object.__new__(m.BuildRun)
        state.channel = m.Channel(Stream(), "a" * 32)
        state.writer_attempted = state.writer_shutdown_ok = False
        state.first, state.secondary = None, []
        state.close_writer()
        state.close_writer()
        self.assertEqual(state.channel.stream.shutdowns, 1)
        self.assertTrue(state.writer_shutdown_ok)
        self.assertIsNone(state.first)

    def test_integer_grammar_is_canonical_and_bounded(self):
        for value in ["", "01", "-1", "1.0", "18446744073709551616"]:
            with self.assertRaises(m.Refused):
                m.integer(value)
        self.assertEqual(m.integer("0"), 0)

    def test_missing_kernel_policy_marker_refuses_before_any_actor_or_file(self):
        from unittest.mock import patch
        with patch.dict(m.os.environ, {}, clear=True), patch.object(m.os, "fork") as fork, patch.object(m, "BuildRun") as owner:
            with self.assertRaisesRegex(m.Refused, "POLICY_PREREQUISITE"):
                m.run({})
        fork.assert_not_called()
        owner.assert_not_called()

    def test_receipt_error_retains_specific_safe_reason(self):
        from unittest.mock import patch
        state = object.__new__(m.BuildRun)
        class Trace:
            def write(self, _):
                raise OSError("private path must not escape")
        state.trace = Trace()
        with patch.object(m.time, "monotonic_ns", return_value=1):
            with self.assertRaisesRegex(m.Refused, "^RECEIPT$"):
                state.event("REGISTERED")

    def test_frame_cap_cannot_trigger_park_before_ninth_minimum(self):
        bodies = ["PARK"] + ["CLEAN|1000"] * 7 + ["CLEAN|100"]
        # This owner accepts only ONE CLEAN. Duplicates remain control failure;
        # neither this different contract nor frame caps permit eager PARK.
        data = wire(*bodies)
        k, g, c = pair([data[:200], data[200:]])
        self.assertFalse(k.turn(c, lambda: 150))
        self.assertEqual(g.events, [])
        self.assertEqual(k.first, "CONTROL")
        self.assertEqual(k.end, 1000)

    def test_private_spec_positive_and_negative_metadata_refusal_before_actors(self):
        import json
        import os
        import tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory(prefix="build-owner-pure-") as temp:
            root = Path(temp).resolve()
            for relative in ["home", "home/.codex", "home/.claude", "home/capture", "home/config", "home/state", "home/AppData", "tmp", "toolchain", "cargo-cache", "rustup-cache", "target"]:
                (root / relative).mkdir(mode=0o700)
            image = root / "toolchain/cargo"
            image.write_text("INERT MARKER; NEVER EXECUTED\n")
            image.chmod(0o700)
            env = {"HOME": str(root / "home"), "CODEX_HOME": str(root / "home/.codex"),
                   "CLAUDE_CONFIG_DIR": str(root / "home/.claude"), "KEITO_CAPTURE_HOME": str(root / "home/capture"),
                   "XDG_CONFIG_HOME": str(root / "home/config"), "XDG_STATE_HOME": str(root / "home/state"),
                   "APPDATA": str(root / "home/AppData"), "TMPDIR": str(root / "tmp"),
                   "PATH": ":".join((str(image.parent), "/usr/bin", "/bin", "/usr/sbin", "/sbin")),
                   "CARGO_HOME": str(root / "cargo-cache"), "RUSTUP_HOME": str(root / "rustup-cache"),
                   "CARGO_TARGET_DIR": str(root / "target"), "CARGO_BUILD_JOBS": "2", "CARGO_NET_OFFLINE": "true",
                   "RUST_TEST_THREADS": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1"}
            base = {"root": str(root), "cwd": str(root), "argv": [str(image), "build", "--offline", "--locked"],
                    "env": env, "seconds": 3, "image_sha256": m.file_hash(image), "owner_sha256": m.file_hash(Path(m.__file__).resolve())}
            path = root / "spec.json"
            def save(value):
                path.write_text(json.dumps(value));path.chmod(0o600)
            save(base)
            with patch.object(m.os, "fork") as fork, patch.object(m.subprocess, "Popen") as popen:
                self.assertEqual(m.spec_read(path)["argv"], base["argv"])
                for key, value, expected in [("HOME", "/outside-private-home", "PRIVATE_ENV"),
                                              ("PATH", "/unapproved/tools", "TOOLCHAIN_PATH"),
                                              ("CARGO_NET_OFFLINE", "false", "BUILD_ENV"),
                                              ("CARGO_TARGET_DIR", str(root.parent), "PRIVATE_TARGET")]:
                    changed = dict(base);changed["env"] = dict(env, **{key:value});save(changed)
                    with self.assertRaisesRegex(m.Refused, expected):
                        m.spec_read(path)
                changed = dict(base, image_sha256="0" * 64);save(changed)
                with self.assertRaisesRegex(m.Refused, "SOURCE_BINDING"):
                    m.spec_read(path)
                changed = dict(base);changed["env"] = dict(env, EXTRA="must-refuse");save(changed)
                with self.assertRaisesRegex(m.Refused, "ENV"):
                    m.spec_read(path)
                path.write_text('{"root":"one","root":"two"}');path.chmod(0o600)
                with self.assertRaisesRegex(m.Refused, "SPEC_DUPLICATE"):
                    m.spec_read(path)
            fork.assert_not_called();popen.assert_not_called()

    def test_parent_success_requires_actual_both_waits_release_and_eof(self):
        from unittest.mock import patch
        state = object.__new__(m.BuildRun)
        state.first, state.secondary = None, []
        state.keeper, state.keeper_status = 42, None
        state.child_status = None
        state.end, state.hard = 1000, 2000
        state.gone = state.release = state.proof = False
        state.writer_attempted = state.writer_shutdown_ok = False
        state.channel = m.Channel(Stream([wire("GONE"), b""]), "a" * 32)
        class Child:
            def poll(self):
                return 0
        state.child = Child()
        def waited(pid, flags):
            self.assertEqual((pid, flags), (42, m.os.WNOHANG))
            return (42, 0) if state.release else (0, 0)
        with patch.object(m.os, "waitpid", side_effect=waited), patch.object(m.time, "monotonic_ns", return_value=1), patch.object(m.time, "sleep"):
            state.settle()
        self.assertTrue(state.proof)
        self.assertEqual((state.child_status, state.keeper_status), (0, 0))
        self.assertEqual(state.channel.stream.output, wire("RELEASE"))
        self.assertEqual(state.channel.stream.shutdowns, 1)

    def test_parent_duplicate_terminal_never_rehabilitates_failed_proof(self):
        from unittest.mock import patch
        state = object.__new__(m.BuildRun)
        state.first, state.secondary = None, []
        state.keeper, state.keeper_status = 42, None
        state.child_status = None
        state.end, state.hard = 1000, 2000
        state.gone = state.release = state.proof = False
        state.writer_attempted = state.writer_shutdown_ok = False
        state.channel = m.Channel(Stream([wire("GONE", "GONE"), b""]), "a" * 32)
        class Child:
            def poll(self):
                return 0
        state.child = Child()
        with patch.object(m.os, "waitpid", side_effect=lambda *_: (42, 0) if state.release else (0, 0)), patch.object(m.time, "monotonic_ns", return_value=1), patch.object(m.time, "sleep"):
            state.settle()
        self.assertFalse(state.proof)
        self.assertEqual(state.first, "TERMINAL")

    def finalizer_case(self, phase):
        import io
        import json
        from contextlib import ExitStack
        from unittest.mock import patch
        state = object.__new__(m.BuildRun)
        state.root = Path("/inert-no-files-opened")
        state.first, state.secondary = "ORIGINAL_OPERATION", []
        state.cancelled = False
        state.keeper, state.keeper_status = 42, None
        state.child_status = None
        state.child_attempted = True
        state.hard, state.end = 1000, 500
        state.gone = state.release = state.proof = False
        state.writer_attempted = state.writer_shutdown_ok = False
        calls, packets = [], []
        class Child:
            pid = 43
            count = 0
            def poll(self):
                self.count += 1
                calls.append("child_wait")
                return 0 if phase != "late" or self.count >= 3 else None
        state.child = Child()
        class ActorStream(Stream):
            def shutdown(self, _):
                calls.append("shutdown")
                if phase == "shutdown":
                    state.cancel_signal(None, None)
                self.shutdowns += 1
        state.channel = m.Channel(ActorStream(), "a" * 32)
        class Close:
            def __init__(self, name):
                self.name = name
            def close(self):
                calls.append(self.name)
                if self.name == "output_close" and phase == "close":
                    state.cancel_signal(None, None)
                    raise OSError("synthetic close")
        state.output, state.trace = Close("output_close"), Close("trace_close")
        parent, child_end = Close("parent_close"), Close("child_end_close")
        class Receipt(io.StringIO):
            def fileno(self):
                return 99
        wait_count = 0
        def wait(*_):
            nonlocal wait_count
            wait_count += 1
            calls.append("keeper_wait")
            return (42, 0) if phase != "late" or wait_count >= 3 else (0, 0)
        def sleep(_):
            calls.append("late_sleep")
            if phase == "late":
                state.cancel_signal(None, None)
        stderr = object()
        def publish(value, **kwargs):
            calls.append("stderr_fact" if kwargs.get("file") is stderr else "stdout_fact")
            if phase == "fact" and kwargs.get("file") is not stderr:
                state.cancel_signal(None, None)
                raise OSError("synthetic output")
            packets.append(json.loads(value))
        error = None
        with ExitStack() as stack:
            stack.enter_context(patch.object(m.os, "waitpid", side_effect=wait))
            stack.enter_context(patch.object(m.os, "fsync"))
            stack.enter_context(patch.object(m.time, "monotonic_ns", return_value=1))
            stack.enter_context(patch.object(m.time, "sleep", side_effect=sleep))
            stack.enter_context(patch.object(m, "exclusive", side_effect=lambda *_: Receipt()))
            stack.enter_context(patch.object(m.signal, "signal", side_effect=lambda *args: calls.append("restore_handler")))
            stack.enter_context(patch.object(m.sys, "stderr", stderr))
            stack.enter_context(patch("builtins.print", side_effect=publish))
            try:
                state.finalize(parent, child_end, False, {m.signal.SIGINT: None, m.signal.SIGTERM: None})
            except BaseException as caught:
                error = caught
        self.assertIsNone(error, "catchable cancellation/secondary errors must not escape finalization")
        self.assertEqual(state.first, "ORIGINAL_OPERATION")
        self.assertEqual((state.child_status, state.keeper_status), (0, 0))
        for name in ["child_wait", "keeper_wait", "child_end_close", "parent_close", "output_close", "trace_close", "restore_handler"]:
            self.assertIn(name, calls)
        self.assertEqual(calls.count("restore_handler"), 2)
        self.assertTrue(packets, "retained facts must reach the bounded fallback")
        self.assertEqual(packets[-1]["owner_result"]["first_fault"], "ORIGINAL_OPERATION")
        self.assertFalse(packets[-1]["first_saved"])
        if phase == "late":
            self.assertGreaterEqual(state.child.count, 3)
            self.assertGreaterEqual(wait_count, 3)
            self.assertIsNone(packets[-1]["cutpoint"]["child_wait"])
            self.assertEqual(packets[-1]["owner_result"]["child_wait"], 0)
        if phase == "fact":
            self.assertIn("stderr_fact", calls)

    def test_r24_cancel_at_finalizer_shutdown_retains_both_waits_and_facts(self):
        self.finalizer_case("shutdown")

    def test_r24_cancel_during_late_wait_does_not_abort_remaining_collection(self):
        self.finalizer_case("late")

    def test_r24_cancel_and_fact_output_failure_still_close_and_publish_fallback(self):
        self.finalizer_case("fact")

    def test_r24_secondary_close_failure_cannot_skip_other_owned_resources(self):
        self.finalizer_case("close")


if __name__ == "__main__":
    unittest.main()
