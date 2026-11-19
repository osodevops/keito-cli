#!/usr/bin/env python3
"""Run Unix integration tests with real, source-built release-pair verification.

No product verifier override: the guard digest is compiled into the test/core,
and each test image that uses capture convenience APIs has its own manifest.
Other tests still use the unpaired Cargo binary to exercise refusal paths.
The macOS caller must already be inside run-macos-isolated.sh's boundary.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

PAIRED_TESTS = {"capture_processes", "capture_store", "capture_transport", "native_capture_guard"}


def digest(path):
    with open(path, "rb") as source:
        return hashlib.sha256(source.read()).hexdigest()


def stage_pair(directory, core, guard, version, target):
    directory.mkdir(mode=0o700)
    for source, name in [(core, "keito"), (guard, "keito-capture-guard")]:
        shutil.copyfile(source, directory / name)
        (directory / name).chmod(0o700)
    manifest = {"manifest_contract": 1, "version": version, "target": target,
                "guard_contract": 2, "files": {
                    name: digest(directory / name) for name in ("keito", "keito-capture-guard")}}
    path = directory / "capture-runtime-manifest.json"
    with path.open("x") as output:
        path.chmod(0o600)
        json.dump(manifest, output)
    return directory / "keito"


def run_test(executable, arguments):
    # Cargo calls this runner for every target; only these four suites call
    # current() from the test executable itself. Never share their manifests.
    name = Path(executable).name.rsplit("-", 1)[0]
    if name in PAIRED_TESTS:
        root = Path(os.environ["KEITO_TEST_PAIR_ROOT"])
        directory = Path(tempfile.mkdtemp(prefix=name + "-", dir=root))
        executable = stage_pair(directory / "pair", executable,
                                root / "guard", os.environ["KEITO_TEST_PAIR_VERSION"],
                                os.environ["KEITO_TEST_PAIR_TARGET"])
    os.execv(str(executable), [str(executable), *arguments])


def main(arguments):
    if arguments[:1] == ["--run"]:
        run_test(arguments[1], arguments[2:])
        return
    cargo, rustc, *test_arguments = arguments
    root = Path(tempfile.mkdtemp(prefix="keito-paired-tests-")).resolve(strict=True)
    root.chmod(0o700)
    print("Source-fixture test pairs: " + str(root), flush=True)
    (root / "tmp").mkdir(mode=0o700)
    os.environ["TMPDIR"] = str(root / "tmp")
    environment = dict(os.environ)
    environment.pop("KEITO_RELEASE_GUARD_SHA256", None)
    # The caller resolves the toolchain before entering this runner. Cargo
    # otherwise finds rustc through PATH, where parallel rustup proxies may
    # attempt component installation during the first guard build.
    environment["RUSTC"] = rustc
    # /usr/bin/git on macOS is an xcrun shim, not the Git image. Resolve the
    # tool once during fixture preparation instead of charging shim discovery
    # to every short owned leaf after env_clear().
    git = (subprocess.check_output(["/usr/bin/xcrun", "--find", "git"],
                                   env=environment, text=True).strip()
           if sys.platform == "darwin" else shutil.which("git", path=environment["PATH"]))
    if not git or not Path(git).is_file():
        raise RuntimeError("fixture Git executable unavailable")
    git_directory = str(Path(git).resolve(strict=True).parent)
    environment["KEITO_TEST_PROCESS_PATH"] = os.pathsep.join([git_directory, "/usr/bin", "/bin"])
    environment["PATH"] = environment["KEITO_TEST_PROCESS_PATH"] + os.pathsep + environment["PATH"]
    # Exercise the production optimization/LTO/codegen footprint without changing
    # fixture checked semantics, dev-tools negative seams, or any runtime bound.
    # DEV/TEST keep their debug assertions and overflow checks explicitly; this
    # remains a source-fixture pair, not the normal release artifact.
    for profile in ("DEV", "TEST"):
        for key, value in {"OPT_LEVEL": "z", "DEBUG": "0", "STRIP": "symbols",
                           "LTO": "true", "CODEGEN_UNITS": "1",
                           "DEBUG_ASSERTIONS": "true", "OVERFLOW_CHECKS": "true"}.items():
            environment["CARGO_PROFILE_" + profile + "_" + key] = value
    environment.setdefault("RUST_TEST_THREADS", "1")
    target = next(line.split(": ", 1)[1] for line in subprocess.check_output(
        [rustc, "-vV"], text=True).splitlines() if line.startswith("host: "))
    version = re.search(r'^version = "([^"]+)"$', Path("Cargo.toml").read_text(), re.M)[1]
    subprocess.run([cargo, "build", "--locked", "--all-features", "--bin",
                    "keito-capture-guard", "--message-format=json"], env=environment,
                   stdout=(root / "guard-build.jsonl").open("x"), check=True)
    guard = artifact(root / "guard-build.jsonl", "keito-capture-guard")
    shutil.copyfile(guard, root / "guard")
    (root / "guard").chmod(0o700)
    environment["KEITO_RELEASE_GUARD_SHA256"] = digest(root / "guard")
    subprocess.run([cargo, "build", "--locked", "--all-features", "--bin", "keito",
                    "--message-format=json"], env=environment,
                   stdout=(root / "core-build.jsonl").open("x"), check=True)
    core = artifact(root / "core-build.jsonl", "keito")
    pair = stage_pair(root / "cli", core, root / "guard", version, target)
    environment.update(KEITO_TEST_PAIR_ROOT=str(root), KEITO_TEST_PAIR_CORE=str(pair),
                       KEITO_TEST_PAIR_VERSION=version, KEITO_TEST_PAIR_TARGET=target)
    runner = shlex.join([sys.executable, "-I", "-S", "-B", str(Path(__file__).resolve()), "--run"])
    environment["CARGO_TARGET_" + target.upper().replace("-", "_") + "_RUNNER"] = runner
    selection = {"--all-targets", "--lib", "--test", "--tests", "--bin", "--bins", "--example", "--examples",
                 "--bench", "--benches", "--doc"}
    targets = [] if selection.intersection(test_arguments) else ["--all-targets"]
    command = [cargo, "test", "--locked", "--all-features", *targets, *test_arguments]
    subprocess.run(command, env=environment, check=True)


def artifact(path, name):
    matches = []
    for line in path.read_text().splitlines():
        value = json.loads(line)
        if (value.get("reason") == "compiler-artifact" and value["target"]["name"] == name
                and value.get("executable") and not value["profile"]["test"]):
            matches.append(value["executable"])
    if len(matches) != 1:
        raise RuntimeError("expected exactly one executable artifact for " + name)
    return matches[0]


if __name__ == "__main__":
    main(sys.argv[1:])
