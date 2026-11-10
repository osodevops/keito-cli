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
from source_qa_cargo import cargo
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
                  "CARGO_HOME": os.environ["CARGO_HOME"],
                  "RUSTUP_HOME": os.environ["RUSTUP_HOME"],
                  "CARGO_TARGET_DIR": str(ROOT / "target-source-only"), "CARGO_NET_OFFLINE": "true",
                  "CARGO_BUILD_JOBS": "2", "RUST_TEST_THREADS": "1", "RUSTFLAGS": "-D warnings",
                  "CARGO_PROFILE_DEV_OPT_LEVEL": "2", "CARGO_PROFILE_DEV_LTO": "false",
                  "CARGO_PROFILE_DEV_CODEGEN_UNITS": "8", "CARGO_PROFILE_DEV_STRIP": "symbols",
                  "CARGO_PROFILE_DEV_DEBUG": "0", "PYTHONDONTWRITEBYTECODE": "1"})


def invoke(label, arguments, timeout, extra=None):
    assert arguments[0] == CARGO and timeout <= 900
    config = ["--config", "profile.dev.opt-level=2", "--config", 'profile.dev.debug=0',
              "--config", "profile.dev.codegen-units=8", "--config", 'profile.dev.strip="symbols"']
    if extra:
        assert set(extra) == {"KEITO_RELEASE_GUARD_SHA256"}
        value = extra["KEITO_RELEASE_GUARD_SHA256"]
        assert len(value) == 64 and all(c in "0123456789abcdef" for c in value)
        config += ["--config", 'env.KEITO_RELEASE_GUARD_SHA256="' + value + '"']
    return cargo(ROOT, REPO, CARGO, BUILD_ENV, label, [*config, *arguments[1:]])


def source_snapshot():
    # Source membership was collected OUTSIDE the inherited policy. No Git or
    # original common-directory access is attempted by this verifier.
    return manifest.verify()


manifest = ManifestLease(REPO, PARENT)
BOUNDARY = ("macOS: existing proved inherited kernel service/network policy" if sys.platform == "darwin"
            else "Linux: private checked operands and environment; no kernel service/network denial claim")
snapshot = source_snapshot()
record("source-before.json", {"source_fixture_only": True, "files": snapshot, "execution_boundary": BOUNDARY,
                              "outer_manifest_sha256": manifest.raw_sha256,
                              "source_set_sha256": manifest.value["source_set_sha256"],
                              "collector": manifest.value["collector"], "deleted_paths": manifest.value["deleted_paths"],
                              "not_final_release_qualification": True})
with manifest:
    import tomllib
    package = tomllib.loads((REPO / "Cargo.toml").read_text())
    version = package["package"]["version"]
    # Pair target comes from Cargo's source-bound build-script output below,
    # never an unowned rustc --version child or a caller-supplied namespace.
    guard_target = invoke("source-guard-build", [CARGO, "build", "--locked", "--offline", "--features", "dev-tools", "--bin", "keito-capture-guard"], 600)
    pair = ROOT / "pair-source-only"
    shutil.copyfile(guard_target / "keito-capture-guard", pair / "keito-capture-guard")
    (pair / "keito-capture-guard").chmod(0o700)
    guard_digest = digest(pair / "keito-capture-guard")
    core_target = invoke("source-core-build", [CARGO, "build", "--locked", "--offline", "--features", "dev-tools", "--bin", "keito"],
           600, {"KEITO_RELEASE_GUARD_SHA256": guard_digest})
    shutil.copyfile(core_target / "keito", pair / "keito")
    (pair / "keito").chmod(0o700)
    target_outputs = list((core_target / "build").glob("keito-cli-*/output"))
    targets = {line.split("=", 2)[2] for path in target_outputs for line in path.read_text().splitlines()
               if line.startswith("cargo:rustc-env=KEITO_BUILD_TARGET=")}
    assert len(targets) == 1
    host = targets.pop()
    pair_manifest = {"manifest_contract": 1, "version": version, "target": host, "guard_contract": 2,
                "files": {"keito": digest(pair / "keito"), "keito-capture-guard": guard_digest}}
    with (pair / "capture-runtime-manifest.json").open("x") as output:
        json.dump(pair_manifest, output, indent=2)
        output.write("\n")
    for name in ("codex-inert-source-only", "claude-inert-source-only"):
        image = ROOT / "owned-native" / name
        fixture_target = invoke("build-" + name, [CARGO, "build", "--locked", "--offline", "--features", "dev-tools", "--bin", name],
               600, {"KEITO_RELEASE_GUARD_SHA256": guard_digest})
        shutil.copyfile(fixture_target / name, image)
        image.chmod(0o700)
    assert source_snapshot() == snapshot, "source changed during source-fixture builds"
    record("pair-and-native-identities.json", {"source_fixture_only": True, "manifest": pair_manifest,
                                             "outer_manifest_sha256": manifest.raw_sha256,
                                             "manifest_sha256": digest(pair / "capture-runtime-manifest.json"),
                                             "native_files": {path.name: digest(path) for path in (ROOT / "owned-native").iterdir()}})
    for label, script, output in (("normal-positive-first", "guarded_usage_acceptance.py", "qa-positives"),
                                   ("normal-negative-first", "guarded_usage_negatives.py", "qa-negatives")):
        # Execute Python acceptance on THIS thread. Every actual runtime Child
        # is retained by its registered multi-slot manager; no opaque subprocess
        # around the whole script and no second source ownership implementation.
        import runpy
        old_argv = sys.argv
        sys.argv = [str(REPO / "tests/support" / script), str(ROOT), str(pair),
                    str(ROOT / "owned-native"), str(ROOT / output)]
        try:
            runpy.run_path(str(REPO / "tests/support" / script), run_name="__main__")
        finally:
            sys.argv = old_argv
    assert source_snapshot() == snapshot, "source changed during source-fixture verification"
    record("source-qa-complete.json", {"source_fixture_only": True, "positive_namespaces": 2,
                                        "negative_cases": 38, "not_real_harness_provider_or_release_qualification": True,
                                        "all_tracked_direct_handles_waited": "see each retained runtime manager receipt and compiler FIRST/FINAL",
                                        "outer_manifest_sha256": manifest.raw_sha256,
                                        "source_set_sha256": manifest.value["source_set_sha256"],
                                        "execution_boundary": BOUNDARY})
    print("Owned normal numeric SOURCE-FIXTURE QA passed; retained at " + str(ROOT))
