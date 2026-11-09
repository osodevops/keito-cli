#!/usr/bin/env python3
"""Bounded inert-only source QA contracts; never executes compiler/product/service."""
from pathlib import Path
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import source_qa_manifest as inventory
from source_qa_children import Cancelled, OwnedChildren

BASE = None


class RetainedDirectory:
    # Evidence, including the first failing case, must survive assertions.
    def __init__(self, prefix, dir):
        self.name = tempfile.mkdtemp(prefix=prefix, dir=dir)

    def cleanup(self):
        pass


PYTHON = str(Path(sys.executable).resolve(strict=True))
LEAF = """import json,os,signal,sys,time
from pathlib import Path
mode,marker=sys.argv[1],Path(sys.argv[2])
if mode=='ignore': signal.signal(signal.SIGTERM,signal.SIG_IGN)
with marker.open('x') as f: json.dump({'pid':os.getpid(),'started_monotonic_ns':time.monotonic_ns(),'term_blocked':signal.SIGTERM in signal.pthread_sigmask(signal.SIG_BLOCK,set())},f)
end=time.monotonic()+8
while time.monotonic()<end: time.sleep(0.05)
"""


def environment(root):
    home = root / "home"
    for relative in ("home", "home/.claude", "home/.codex", "home/config", "home/capture", "home/state", "home/AppData", "tmp", "bin"):
        (root / relative).mkdir(parents=True, exist_ok=True, mode=0o700)
    return {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "CLAUDE_CONFIG_DIR": str(home / ".claude"),
            "KEITO_CAPTURE_HOME": str(home / "capture"), "XDG_CONFIG_HOME": str(home / "config"),
            "XDG_STATE_HOME": str(home / "state"), "APPDATA": str(home / "AppData"), "TMPDIR": str(root / "tmp"),
            "PATH": str(root / "bin"), "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null",
            "PYTHONDONTWRITEBYTECODE": "1"}


def child_arguments(root, mode="normal"):
    return [PYTHON, "-I", "-S", "-c", LEAF, mode, str(root / "leaf-started.json")]


class ManifestFixture:
    def __init__(self):
        self.temp = RetainedDirectory(prefix="manifest-", dir=BASE)
        self.root = Path(self.temp.name).resolve(strict=True)
        self.repo, self.parent = self.root / "repo", self.root / "parent"
        self.repo.mkdir(mode=0o700)
        self.parent.mkdir(mode=0o700)
        (self.repo / "tests/support").mkdir(parents=True, mode=0o700)
        (self.repo / "src").mkdir(mode=0o700)
        shutil.copyfile(Path(inventory.__file__), self.repo / inventory.COLLECTOR_NAME)
        (self.repo / "src/inert.rs").write_text("// inert source data only\n")
        names = [inventory.COLLECTOR_NAME, "src/inert.rs"]
        deleted = ["tests/removed.rs"]
        files = inventory.snapshot(self.repo, names, deleted)
        self.value = {"schema_version": 1, "repo": str(self.repo), "head_sha": "a" * 40,
                      "collector": {"name": inventory.COLLECTOR_NAME, "sha256": files[inventory.COLLECTOR_NAME]["sha256"]},
                      "completeness": inventory.COMPLETENESS, "files": files, "deleted_paths": deleted,
                      "source_set_sha256": inventory.source_set_digest(files, deleted), "created_utc": "inert"}

    def write(self):
        self.value["source_set_sha256"] = inventory.source_set_digest(self.value["files"], self.value["deleted_paths"])
        path = self.parent / inventory.MANIFEST_NAME
        path.write_text(json.dumps(self.value))
        path.chmod(0o600)
        return path

    def close(self):
        self.temp.cleanup()


class ManifestTests(unittest.TestCase):
    def test_all_unsafe_manifest_cases_deny_before_compiler_marker(self):
        for case in ("missing", "malformed", "oversize", "wrong-repo", "unsafe-name", "changed-source", "missing-source",
                     "source-symlink", "parent-symlink", "deleted-reappeared", "collector-mismatch", "manifest-mode",
                     "manifest-symlink", "manifest-hardlink", "parent-mode", "unknown-field", "duplicate-field"):
            with self.subTest(case=case):
                fixture = ManifestFixture()
                try:
                    path = fixture.write()
                    if case == "missing": path.unlink()
                    elif case == "malformed": path.write_text("{")
                    elif case == "oversize": path.write_bytes(b" " * (inventory.MAX_MANIFEST_BYTES + 1))
                    elif case == "wrong-repo": fixture.value["repo"] = "/inert/foreign"; fixture.write()
                    elif case == "unsafe-name": fixture.value["files"]["../escape"] = fixture.value["files"].pop("src/inert.rs"); fixture.write()
                    elif case == "changed-source": (fixture.repo / "src/inert.rs").write_text("changed")
                    elif case == "missing-source": (fixture.repo / "src/inert.rs").unlink()
                    elif case == "source-symlink":
                        source = fixture.repo / "src/inert.rs"; source.rename(source.with_suffix(".previous")); source.symlink_to(source.with_suffix(".previous"))
                    elif case == "parent-symlink":
                        source = fixture.repo / "src"; source.rename(fixture.repo / "src-previous"); source.symlink_to(fixture.repo / "src-previous")
                    elif case == "deleted-reappeared": (fixture.repo / "tests/removed.rs").write_text("reappeared")
                    elif case == "collector-mismatch": fixture.value["collector"]["sha256"] = "0" * 64; fixture.write()
                    elif case == "manifest-mode": path.chmod(0o644)
                    elif case == "manifest-symlink":
                        saved = path.with_suffix(".previous"); path.rename(saved); path.symlink_to(saved)
                    elif case == "manifest-hardlink": os.link(path, path.with_suffix(".link"))
                    elif case == "parent-mode": fixture.parent.chmod(0o755)
                    elif case == "unknown-field": fixture.value["unknown"] = True; fixture.write()
                    elif case == "duplicate-field": path.write_text(path.read_text()[:-1] + ',"schema_version":1}')
                    marker = fixture.root / "compiler-must-not-be-constructed"
                    try:
                        with inventory.ManifestLease(fixture.repo, fixture.parent) as lease:
                            lease.verify()
                            marker.write_text("unsafe")
                    except (inventory.ManifestError, OSError, ValueError, TypeError):
                        pass
                    self.assertFalse(marker.exists(), case)
                finally:
                    fixture.close()

    def test_manifest_file_source_and_deleted_paths_are_rechecked_after_admission(self):
        for case in ("manifest", "source", "deleted"):
            fixture = ManifestFixture()
            try:
                path = fixture.write()
                with inventory.ManifestLease(fixture.repo, fixture.parent) as lease:
                    if case == "manifest": path.write_text(path.read_text() + " ")
                    elif case == "source": (fixture.repo / "src/inert.rs").write_text("changed")
                    else: (fixture.repo / "tests/removed.rs").write_text("changed")
                    with self.assertRaises((inventory.ManifestError, OSError)):
                        lease.verify()
            finally:
                fixture.close()


class ReceiptFault(RuntimeError):
    pass


class FaultyReceipts(OwnedChildren):
    def _record(self, name, value):
        if name.endswith("-child-started.json"):
            super()._record(name, value)
            raise ReceiptFault("inert injected start receipt failure")
        if name.endswith("-result.json"):
            raise RuntimeError("inert secondary result receipt failure")
        super()._record(name, value)


class ChildTests(unittest.TestCase):
    def case(self):
        temp = RetainedDirectory(prefix="child-", dir=BASE)
        root = Path(temp.name).resolve(strict=True)
        return temp, root, environment(root)

    def test_fast_owned_success_is_reaped_without_any_group_signal(self):
        temp, root, env = self.case()
        try:
            with OwnedChildren(root, env, root) as manager:
                output = manager.run("success", [PYTHON, "-I", "-S", "-c", "print('inert')"], 10, capture_limit=128)
                self.assertIsNone(manager.active)
            result = json.loads((root / "success-result.json").read_text())
            self.assertEqual(output, b"inert\n")
            self.assertTrue(result["waited"])
            self.assertEqual(result["returncode"], 0)
            self.assertNotIn("cleanup", result["child"])
        finally:
            temp.cleanup()

    def test_timeout_terminates_unblocked_owned_group_and_waits(self):
        temp, root, env = self.case()
        try:
            with OwnedChildren(root, env, root) as manager:
                with self.assertRaises(subprocess.TimeoutExpired):
                    manager.run("timeout", child_arguments(root), 1)
                self.assertIsNone(manager.active)
            started = json.loads((root / "timeout-child-started.json").read_text())
            result = json.loads((root / "timeout-result.json").read_text())
            leaf = json.loads((root / "leaf-started.json").read_text())
            self.assertEqual(started["pid"], leaf["pid"])
            self.assertFalse(leaf["term_blocked"])
            self.assertTrue(result["waited"])
            self.assertEqual(result["returncode"], -signal.SIGTERM)
            self.assertEqual([value["signal"] for value in result["child"]["cleanup"]["actions"]], [signal.SIGTERM])
        finally:
            temp.cleanup()

    def test_term_ignoring_owned_child_requires_bounded_kill_then_wait(self):
        temp, root, env = self.case()
        try:
            with OwnedChildren(root, env, root) as manager:
                with self.assertRaises(subprocess.TimeoutExpired):
                    manager.run("kill", child_arguments(root, "ignore"), 1)
                self.assertIsNone(manager.active)
            result = json.loads((root / "kill-result.json").read_text())
            self.assertTrue(result["waited"])
            self.assertEqual(result["returncode"], -signal.SIGKILL)
            self.assertEqual([value["signal"] for value in result["child"]["cleanup"]["actions"]], [signal.SIGTERM, signal.SIGKILL])
        finally:
            temp.cleanup()

    def test_receipt_exception_still_cleans_waits_and_preserves_original_error(self):
        temp, root, env = self.case()
        try:
            with FaultyReceipts(root, env, root) as manager:
                with self.assertRaises(ReceiptFault):
                    manager.run("receipt", child_arguments(root), 10)
                self.assertIsNone(manager.active)
            result = json.loads((root / "receipt-cleanup-fallback.json").read_text())
            self.assertTrue(result["waited"])
            self.assertEqual(result["error_type"], "ReceiptFault")
            self.assertTrue((root / "receipt-child-started.json").is_file())
        finally:
            temp.cleanup()

    def test_sigterm_to_owned_runner_finalizes_its_detached_child_before_exit(self):
        temp, root, env = self.case()
        try:
            host = root / "host"
            host.mkdir(mode=0o700)
            environment(host)
            arguments = [PYTHON, "-I", "-S", str(Path(__file__).resolve()), "--signal-host", str(host)]
            with OwnedChildren(root, env, root) as manager:
                with self.assertRaises(subprocess.TimeoutExpired):
                    manager.run("host", arguments, 1)
                self.assertIsNone(manager.active)
            outer = json.loads((root / "host-result.json").read_text())
            inner = json.loads((host / "signal-child-result.json").read_text())
            cancellation = json.loads((host / "host-cancelled.json").read_text())
            self.assertTrue(outer["waited"] and inner["waited"])
            self.assertEqual(outer["returncode"], 128 + signal.SIGTERM)
            self.assertEqual(inner["cancel_signal"], signal.SIGTERM)
            self.assertEqual(inner["returncode"], -signal.SIGTERM)
            self.assertTrue(cancellation["direct_handle_cleared_after_wait"])
        finally:
            temp.cleanup()


def signal_host(root):
    root = inventory.canonical_root(root, private=True)
    manager = OwnedChildren(root, environment(root), root)
    try:
        with manager:
            manager.run("signal-child", child_arguments(root), 10)
    except Cancelled as failure:
        with (root / "host-cancelled.json").open("x") as output:
            json.dump({"signal": failure.signum, "direct_handle_cleared_after_wait": manager.active is None}, output)
        raise SystemExit(128 + failure.signum)
    raise SystemExit("inert cancellation was not observed")


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--signal-host":
        signal_host(Path(sys.argv[2]))
    assert len(sys.argv) == 2, "test_source_qa_contract.py <canonical private parent>"
    parent = inventory.canonical_root(sys.argv[1], private=True)
    BASE = parent / "source-qa-contract-inert"
    BASE.mkdir(mode=0o700)
    unittest.main(argv=[sys.argv[0]], verbosity=2)
