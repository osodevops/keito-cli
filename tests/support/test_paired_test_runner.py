"""Dependency-free fixture regressions; never launches Cargo or a native client."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("paired", Path(__file__).with_name("run_paired_tests.py"))
paired = importlib.util.module_from_spec(spec)
spec.loader.exec_module(paired)


class PairFixtures(unittest.TestCase):
    def test_default_and_explicit_all_targets_are_passed_exactly_once(self):
        for arguments in ([], ["--all-targets"]):
            with self.subTest(arguments=arguments), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                run = root / "run"
                run.mkdir()
                for name in ("git", "guard-bin", "core-bin"):
                    (root / name).write_bytes(name.encode())
                with patch.dict(paired.os.environ, {"RUSTC": "fixture-inherited-rustup-proxy"}), patch.object(paired.sys, "platform", "linux"), \
                        patch.object(paired.tempfile, "mkdtemp", return_value=str(run)), \
                        patch.object(paired.shutil, "which", return_value=str(root / "git")), \
                        patch.object(paired.subprocess, "check_output", return_value="host: fixture-target\n"), \
                        patch.object(paired, "artifact", side_effect=[root / "guard-bin", root / "core-bin"]), \
                        patch.object(paired.subprocess, "run") as launch:
                    paired.main(["fixture-cargo", "fixture-rustc", *arguments])
                command = launch.call_args_list[-1].args[0]
                self.assertEqual(command.count("--all-targets"), 1)
                # Cargo must use the compiler passed by the caller for every
                # phase, never the rustup shim found through PATH.
                for call in launch.call_args_list:
                    self.assertEqual(call.kwargs["env"].get("RUSTC"), "fixture-rustc")
                    self.assertIn("--all-features", call.args[0])
                    for profile in ("DEV", "TEST"):
                        for key, value in {"OPT_LEVEL": "s", "DEBUG": "0",
                                           "STRIP": "symbols", "LTO": "true",
                                           "CODEGEN_UNITS": "1", "DEBUG_ASSERTIONS": "true",
                                           "OVERFLOW_CHECKS": "true"}.items():
                            self.assertEqual(call.kwargs["env"].get(
                                "CARGO_PROFILE_" + profile + "_" + key), value)

    def test_each_running_image_has_its_own_binding_and_tampering_breaks_it(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, content in [("one", b"first-image"), ("two", b"second-image"),
                                  ("guard", b"exact-compiled-guard")]:
                (root / name).write_bytes(content)
            manifests = []
            for image in ("one", "two"):
                core = paired.stage_pair(root / (image + "-pair"), root / image,
                                         root / "guard", "0.2.0", "fixture-target")
                manifest = json.loads((core.parent / "capture-runtime-manifest.json").read_text())
                self.assertEqual(manifest["files"]["keito"], paired.digest(root / image))
                self.assertEqual(manifest["files"]["keito-capture-guard"], paired.digest(root / "guard"))
                self.assertEqual(core.stat().st_mode & 0o777, 0o700)
                manifests.append(manifest)
            self.assertNotEqual(manifests[0]["files"]["keito"], manifests[1]["files"]["keito"])
            core.write_bytes(b"changed-image")
            self.assertNotEqual(manifest["files"]["keito"], paired.digest(core))

    def test_only_the_normal_binary_can_supply_the_core_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "artifacts.jsonl"
            rows = [{"reason": "compiler-artifact", "target": {"name": "keito"},
                     "executable": name, "profile": {"test": test}}
                    for name, test in [("unit-test", True), ("core", False)]]
            path.write_text("\n".join(map(json.dumps, rows)))
            self.assertEqual(paired.artifact(path, "keito"), "core")
            path.write_text(json.dumps(rows[0]))
            with self.assertRaises(RuntimeError):
                paired.artifact(path, "keito")
            rows.append({**rows[1], "executable": "ambiguous"})
            path.write_text("\n".join(map(json.dumps, rows)))
            with self.assertRaises(RuntimeError):
                paired.artifact(path, "keito")


if __name__ == "__main__":
    unittest.main()
