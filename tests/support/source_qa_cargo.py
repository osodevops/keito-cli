"""Use the separately reviewed opaque COMPILER owner, never runtime Scope.

Only preparation/argv changed here; source_qa_build_owner.py stays unchanged.
Compiler-tree cancellation remains outside the runtime qualification claim.
"""
from pathlib import Path
import hashlib
import json
import os


def cargo(root, repo, image, base_environment, label, arguments):
    from source_qa_build_owner import run, spec_read
    import source_qa_build_owner as owner_source
    assert label and all(c.isalnum() or c == "-" for c in label)
    image = Path(image).resolve(strict=True)
    assert image.name == "cargo" and image.is_file()
    build = root / ("compiler-" + label)
    build.mkdir(mode=0o700)
    for relative in ("home", "home/.codex", "home/.claude", "home/config", "home/state",
                     "home/capture", "home/AppData", "tmp", "target"):
        (build / relative).mkdir(mode=0o700)
    env = {"HOME": str(build / "home"), "CODEX_HOME": str(build / "home/.codex"),
           "CLAUDE_CONFIG_DIR": str(build / "home/.claude"), "KEITO_CAPTURE_HOME": str(build / "home/capture"),
           "XDG_CONFIG_HOME": str(build / "home/config"), "XDG_STATE_HOME": str(build / "home/state"),
           "APPDATA": str(build / "home/AppData"), "TMPDIR": str(build / "tmp"),
           "PATH": ":".join((str(image.parent), "/usr/bin", "/bin", "/usr/sbin", "/sbin")),
           "CARGO_HOME": base_environment["CARGO_HOME"], "RUSTUP_HOME": base_environment["RUSTUP_HOME"],
           "CARGO_TARGET_DIR": str(build / "target"), "CARGO_BUILD_JOBS": "2",
           "CARGO_NET_OFFLINE": "true", "RUST_TEST_THREADS": "1",
           "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1"}
    digest = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
    spec = {"root": str(build), "cwd": str(repo), "argv": [str(image), *arguments], "env": env,
            "seconds": 600, "image_sha256": digest(image), "owner_sha256": digest(owner_source.__file__)}
    spec_path = build / "spec.json"
    fd = os.open(spec_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(spec, output, indent=2); output.write("\n"); output.flush(); os.fsync(output.fileno())
    old = os.environ.get("KEITO_BOOTSTRAP_POLICY_CONFIRMED")
    os.environ["KEITO_BOOTSTRAP_POLICY_CONFIRMED"] = "source-group-build-only"
    try:
        if not run(spec_read(spec_path)):
            raise RuntimeError("first compiler command failed; preserve evidence")
    finally:
        if old is None:
            os.environ.pop("KEITO_BOOTSTRAP_POLICY_CONFIRMED", None)
        else:
            os.environ["KEITO_BOOTSTRAP_POLICY_CONFIRMED"] = old
    return build / "target/debug"

