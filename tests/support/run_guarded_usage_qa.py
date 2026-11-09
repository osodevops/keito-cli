#!/usr/bin/env python3
"""Build and verify an isolated opt2 SOURCE-FIXTURE pair, never a release candidate.

Run inside the existing macOS inherited test boundary, or the Linux source-test
job. Linux uses private checked operands/environment and makes no kernel service
or network denial claim. Every product Command passes actual Rust isolation::checked.
No real harness, credentials, trust override, provider or service is used.
"""
from pathlib import Path
import hashlib
import json
import os
import shutil
import stat
import sys

sys.dont_write_bytecode = True
from source_qa_children import OwnedChildren
from source_qa_manifest import ManifestLease
assert sys.platform in ("darwin", "linux"), "Unix source QA is unsupported here"
assert len(sys.argv) == 4, "run_guarded_usage_qa.py <private-run-root> <cargo> <rustc>"
REPO = Path(__file__).resolve().parents[2]
PARENT = Path(sys.argv[1])
assert PARENT.is_absolute() and PARENT.resolve(strict=True) == PARENT
metadata = PARENT.lstat()
assert stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.getuid()
assert stat.S_IMODE(metadata.st_mode) == 0o700
ROOT = PARENT / "guarded-usage-source-qa"
ROOT.mkdir(mode=0o700)  # Never replace an earlier run or its first observations.
os.umask(0o077)
for relative in ("home", "home/.codex", "home/.claude", "home/capture", "home/config",
                 "home/state", "home/AppData", "tmp", "target-source-only", "owned-native",
                 "pair-source-only", "qa-positives", "qa-negatives"):
    (ROOT / relative).mkdir(parents=True, exist_ok=True, mode=0o700)
CARGO, RUSTC = sys.argv[2:]
assert Path(CARGO).is_absolute() and Path(RUSTC).is_absolute()


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(64 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def record(name, value):
    with (ROOT / name).open("x") as output:
        json.dump(value, output, indent=2)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())


if sys.platform == "darwin":
    # The caller has already proved and inherited the single kernel policy.
    # This runner must never launch a second sandbox or relax its restrictions.
    receipt = Path(os.environ["KEITO_TEST_SERVICE_BOUNDARY_RECEIPT"])
    assert json.loads(receipt.read_text())["passed"] is True

BUILD_ENV = {key: os.environ[key] for key in
             ("PATH", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN",
              "KEITO_TEST_SERVICE_BOUNDARY_RECEIPT") if key in os.environ}
BUILD_ENV.update({"HOME": str(ROOT / "home"), "CODEX_HOME": str(ROOT / "home/.codex"),
                  "CLAUDE_CONFIG_DIR": str(ROOT / "home/.claude"),
                  "KEITO_CAPTURE_HOME": str(ROOT / "home/capture"),
                  "XDG_CONFIG_HOME": str(ROOT / "home/config"),
                  "XDG_STATE_HOME": str(ROOT / "home/state"), "APPDATA": str(ROOT / "home/AppData"),
                  "TMPDIR": str(ROOT / "tmp"), "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null",
                  "CARGO_HOME": os.environ.get("CARGO_HOME", str(Path(os.environ["HOME"]) / ".cargo")),
                  "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path(os.environ["HOME"]) / ".rustup")),
                  "CARGO_TARGET_DIR": str(ROOT / "target-source-only"), "CARGO_NET_OFFLINE": "true",
                  "CARGO_BUILD_JOBS": "2", "RUST_TEST_THREADS": "1", "RUSTFLAGS": "-D warnings",
                  "CARGO_PROFILE_DEV_OPT_LEVEL": "2", "CARGO_PROFILE_DEV_LTO": "false",
                  "CARGO_PROFILE_DEV_CODEGEN_UNITS": "8", "CARGO_PROFILE_DEV_STRIP": "symbols",
                  "CARGO_PROFILE_DEV_DEBUG": "0", "PYTHONDONTWRITEBYTECODE": "1"})


def invoke(label, arguments, timeout, extra=None):
    manager.run(label, arguments, timeout, extra=extra)


def source_snapshot():
    # Source membership was collected OUTSIDE the inherited policy. No Git or
    # original common-directory access is attempted by this verifier.
    return manifest.verify()


manifest = ManifestLease(REPO, PARENT)
manager = OwnedChildren(ROOT, BUILD_ENV, REPO)
BOUNDARY = ("macOS: existing proved inherited kernel service/network policy" if sys.platform == "darwin"
            else "Linux: private checked operands and environment; no kernel service/network denial claim")
snapshot = source_snapshot()
record("source-before.json", {"source_fixture_only": True, "files": snapshot, "execution_boundary": BOUNDARY,
                              "outer_manifest_sha256": manifest.raw_sha256,
                              "source_set_sha256": manifest.value["source_set_sha256"],
                              "collector": manifest.value["collector"], "deleted_paths": manifest.value["deleted_paths"],
                              "not_final_release_qualification": True})
with manifest, manager:
    metadata = json.loads(manager.run("source-metadata", [CARGO, "metadata", "--locked", "--offline", "--no-deps", "--format-version", "1"],
                                      60, capture_limit=2 * 1024 * 1024))
    version = next(package["version"] for package in metadata["packages"] if package["name"] == "keito-cli")
    host = next(line.removeprefix("host: ") for line in
                manager.run("rustc-version", [RUSTC, "-vV"], 30, capture_limit=8192).decode().splitlines() if line.startswith("host: "))
    invoke("source-guard-build", [CARGO, "build", "--locked", "--offline", "--features", "dev-tools", "--bin", "keito-capture-guard"], 900)
    pair = ROOT / "pair-source-only"
    shutil.copyfile(ROOT / "target-source-only/debug/keito-capture-guard", pair / "keito-capture-guard")
    (pair / "keito-capture-guard").chmod(0o700)
    guard_digest = digest(pair / "keito-capture-guard")
    invoke("source-core-build", [CARGO, "build", "--locked", "--offline", "--features", "dev-tools", "--bin", "keito"],
           900, {"KEITO_RELEASE_GUARD_SHA256": guard_digest})
    shutil.copyfile(ROOT / "target-source-only/debug/keito", pair / "keito")
    (pair / "keito").chmod(0o700)
    pair_manifest = {"manifest_contract": 1, "version": version, "target": host, "guard_contract": 1,
                "files": {"keito": digest(pair / "keito"), "keito-capture-guard": guard_digest}}
    with (pair / "capture-runtime-manifest.json").open("x") as output:
        json.dump(pair_manifest, output, indent=2)
        output.write("\n")
    for name, version in (("codex-inert-source-only", "0.167.0"), ("claude-inert-source-only", "2.1.319")):
        image = ROOT / "owned-native" / name
        invoke("build-" + name, [RUSTC, "--edition=2021", "-D", "warnings",
                                 str(REPO / "tests/fixtures/capture_native_origin.rs"), "-o", str(image)],
               120, {"OWNED_SOURCE_FIXTURE_VERSION": version})
        image.chmod(0o700)
    assert source_snapshot() == snapshot, "source changed during source-fixture builds"
    record("pair-and-native-identities.json", {"source_fixture_only": True, "manifest": pair_manifest,
                                             "outer_manifest_sha256": manifest.raw_sha256,
                                             "manifest_sha256": digest(pair / "capture-runtime-manifest.json"),
                                             "native_files": {path.name: digest(path) for path in (ROOT / "owned-native").iterdir()}})
    for label, script, output in (("normal-positive-first", "guarded_usage_acceptance.py", "qa-positives"),
                                   ("normal-negative-first", "guarded_usage_negatives.py", "qa-negatives")):
        invoke(label, [sys.executable, str(REPO / "tests/support" / script), str(ROOT), str(pair),
                       str(ROOT / "owned-native"), str(ROOT / output)], 600)
    assert source_snapshot() == snapshot, "source changed during source-fixture verification"
    record("source-qa-complete.json", {"source_fixture_only": True, "positive_namespaces": 2,
                                        "negative_cases": 38, "not_real_harness_provider_or_release_qualification": True,
                                        "all_tracked_direct_handles_waited": manager.active is None,
                                        "outer_manifest_sha256": manifest.raw_sha256,
                                        "source_set_sha256": manifest.value["source_set_sha256"],
                                        "execution_boundary": BOUNDARY})
    print("Owned normal numeric SOURCE-FIXTURE QA passed; retained at " + str(ROOT))
