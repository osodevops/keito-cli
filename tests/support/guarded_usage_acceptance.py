#!/usr/bin/env python3
"""Developer-only normal-route SOURCE-FIXTURE acceptance, never production code.

Requires an already built, truly embedded-anchor owned pair and owned inert Rust
native fixtures. No synthetic Rust approval constructor, provider, auth, actual
harness, original profile or release-candidate qualification is involved.
Run inside the one proved inherited source-test kernel policy, never nested.
"""
from pathlib import Path
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import time
import uuid

ROOT, PAIR, NATIVES, OUTPUT = (Path(value) for value in sys.argv[1:])
for path in [ROOT, PAIR, NATIVES, OUTPUT]:
    assert path.is_absolute() and path.resolve(strict=True) == path
    assert path.stat().st_uid == os.getuid() and path.stat().st_mode & 0o777 == 0o700
assert PAIR.is_relative_to(ROOT) and NATIVES.is_relative_to(ROOT) and OUTPUT.is_relative_to(ROOT)
CORE = PAIR / "keito"
CONTROLLER = NATIVES / "codex-inert-source-only"
assert CORE.is_file() and CORE.resolve() == CORE
sequence = 0


def sha(value):
    return hashlib.sha256(value).hexdigest()


def observe(value):
    global sequence
    sequence += 1
    with (OUTPUT / f"{sequence:04d}.json").open("x") as output:
        json.dump({"source_fixture_only": True, **value}, output, indent=2)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())


def environment(case):
    home = case / "home"
    return {"HOME": str(home), "CODEX_HOME": str(home / ".codex"),
            "CLAUDE_CONFIG_DIR": str(home / ".claude"),
            "KEITO_CAPTURE_HOME": str(home / "capture"),
            "XDG_CONFIG_HOME": str(home / "config"), "XDG_STATE_HOME": str(home / "state"),
            "APPDATA": str(home / "AppData"), "TMPDIR": str(case / "tmp"),
            "PATH": str(case / "bin"), "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null"}


def cli(case, arguments):
    # The owned Rust --cli gateway calls actual isolation::checked plus inert
    # missing-operand negatives before it constructs the product Command.
    result = subprocess.run([str(CONTROLLER), "--cli", str(ROOT), str(case), str(CORE), *arguments],
                            env=environment(case), cwd=case, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=15)
    return result


def make_case(namespace):
    case = ROOT / ("numeric-" + namespace + "-" + uuid.uuid4().hex)
    case.mkdir(mode=0o700)
    for relative in ["home", "home/.claude", "home/.codex", "home/capture", "home/config",
                     "home/state", "home/AppData", "bin", "tmp", "repo", "escape"]:
        path = case / relative
        path.mkdir(parents=True, exist_ok=True)
        path.chmod(0o700)
        assert path.resolve() == path and path.stat().st_uid == os.getuid()
    for name, path in [("git", Path("/usr/bin/git")), ("bash", Path("/bin/bash")),
                       ("sleep", Path("/bin/sleep")), ("codex", CONTROLLER),
                       ("claude", NATIVES / "claude-inert-source-only")]:
        (case / "bin" / name).symlink_to(path.resolve(strict=True))
    result = cli(case, ["skill", "runtime", "--json"])
    assert result.returncode == 0, "owned embedded-anchor source pair unavailable"
    result = cli(case, ["--quiet", "skill", "install", "--source", "bundled",
                        "--agent", "codex", "--agent", "claude-code"])
    observe({"namespace": namespace, "case": "quiet-private-install", "exit": result.returncode})
    assert result.returncode == 0 and not result.stdout, "private managed installation failed"
    git = subprocess.run(["/usr/bin/git", "init", "--quiet"], cwd=case / "repo",
                         env=environment(case), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    assert git.returncode == 0
    result = cli(case, ["capture", "status", "--json"])
    assert result.returncode == 0
    database = sqlite3.connect(case / "home/capture/capture.sqlite3")
    binding = {"repository_key": sha(str((case / "repo/.git").resolve()).encode()),
               "remote_hash": sha(b""), "hint_hash": sha(b"no-mapping-hint"),
               "id": sha(("synthetic-binding-" + namespace).encode()), "version": 1,
               "workspace_id": "co_synthetic", "owner_id": "owner_synthetic",
               "origin_hash": sha(b"synthetic-origin"), "project_id": "project_synthetic",
               "task_id": "task_synthetic", "agents": ["codex", "claude-code"],
               "paused": False, "revoked": False}
    database.execute("INSERT INTO bindings(repo_key,data) VALUES(?,?)",
                     (binding["repository_key"], json.dumps(binding)))
    database.commit()
    return case, database


def write_source(path, records):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(record) + "\n" for record in records))
    path.chmod(0o600)


def dispatch(case, namespace, session, source, event, turn, *, program_override=None, chain_depth=None, overrides=None, remove_fields=()):
    config = json.loads((case / ("home/.codex/hooks.json" if namespace == "codex"
                                 else "home/.claude/settings.json")).read_text())
    native_event = "UserPromptSubmit" if event == "prompt" else "Stop"
    command = config["hooks"][native_event][0]["hooks"][0]["command"]
    label = event + "-" + uuid.uuid4().hex
    command_file, event_file, result_file = (case / (label + suffix)
                                           for suffix in ["-command.txt", "-input.json", "-result.json"])
    command_file.write_text(command)
    command_file.chmod(0o600)
    body = {"session_id": session, "turn_id": turn, "prompt_id": turn,
            "cwd": str(case / "repo"), "transcript_path": str(source),
            "prompt": "PRIVATE_NORMAL_PROMPT_CANARY"}
    if overrides:
        body.update(overrides)
    for field in remove_fields:
        body.pop(field, None)
    event_file.write_text(json.dumps(body))
    event_file.chmod(0o600)
    program = program_override or (CONTROLLER if namespace == "codex" else NATIVES / "claude-inert-source-only")
    mode = ["--run"] if chain_depth is None else ["--chain", str(chain_depth)]
    started = time.monotonic()
    result = subprocess.run([str(program), *mode, str(ROOT), str(case), str(CORE),
                             str(command_file), str(event_file), str(result_file)],
                            cwd=case, env=environment(case), stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=5)
    outcome = json.loads(result_file.read_text()) if result_file.exists() else {"missing_result": True}
    flags = list((case / "home/capture/capture-invocations").glob("*.pending"))
    flags += list((case / "home/capture/capture-invocations").glob("*.preparing"))
    observe({"namespace": namespace, "case": "normal-guarded-" + event, "exit": result.returncode,
             "outcome": outcome, "pending_flags": len(flags),
             "outer_elapsed_ms": (time.monotonic() - started) * 1000})
    assert result.returncode == 0 and not outcome.get("host_timeout", True)
    assert outcome["elapsed_ms"] < 1000 and not flags, "retain genuine guard/ACK failure before numeric assertions"


def main():
    failures = []
    for namespace, version, filename in [("codex", "0.167.0", "codex-inert-source-only"),
                                          ("claude-code", "2.1.319", "claude-inert-source-only")]:
        case, database = make_case(namespace)
        session = str(uuid.uuid4())
        if namespace == "codex":
            source = case / "home/.codex/sessions/2026/11/08" / ("rollout-synthetic-" + session + ".jsonl")
            meta = {"type": "session_meta", "payload": {"id": session, "session_id": session, "cli_version": version}}
            context = lambda turn: {"type": "turn_context", "payload": {"turn_id": turn, "model": "gpt-6.1-sol"}}
            response = lambda identity, turn, usage: {"type": "token_usage_record", "payload": {
                "thread_id": session, "session_id": session, "turn_id": turn, "root_turn_id": turn,
                "response_id": identity, "usage": usage}}
            old_id, new_id = "resp_before_consent", "resp_after_consent"
            initial = [meta, context("old_turn"), response(old_id, "old_turn", None)]
            final = [meta, context("old_turn"), response(old_id, "old_turn", {"input_tokens": "PRIVATE_OLD_COUNTER_CANARY"}),
                     context("turn_synthetic"), response(new_id, "turn_synthetic", {
                         "input_tokens": 101, "cached_input_tokens": 80, "cache_write_input_tokens": 0,
                         "output_tokens": 5, "reasoning_output_tokens": 2})]
        else:
            source = case / "home/.claude/projects/synthetic" / (session + ".jsonl")
            response = lambda identity, usage: {"type": "assistant", "sessionId": session, "message": {
                "id": identity, "model": "claude-opus-5-5", "stop_reason": "end_turn", "usage": usage,
                "content": [{"text": "PRIVATE_NORMAL_SOURCE_CANARY"}]}}
            old_id, new_id = "msg_before_consent", "msg_after_consent"
            initial = [response(old_id, {"input_tokens": "PRIVATE_OLD_COUNTER_CANARY"})]
            final = initial + [response(new_id, {"input_tokens": 2, "cache_read_input_tokens": None,
                                                 "cache_creation_input_tokens": 2280, "output_tokens": 5})]
        write_source(source, initial)
        before_enable = source.read_bytes()
        result = cli(case, ["capture", "usage", "enable", "--repo", "--agent", namespace,
                            "--profile-home", str(case / "home"), "--harness-executable", str(NATIVES / filename),
                            "--native-executable", str(NATIVES / filename), "--confirm", "--json"])
        observe({"namespace": namespace, "case": "explicit-normal-enable", "exit": result.returncode,
                 "stdout_bytes": len(result.stdout), "source_bytes_unchanged": source.read_bytes() == before_enable})
        assert result.returncode == 0, "normal numeric consent publication failed before factory acceptance"
        enabled = json.loads(result.stdout)
        assert enabled["enabled"] is True and enabled["status"] == "UNAVAILABLE"
        policy = json.loads(database.execute("SELECT data FROM usage_consents WHERE namespace=?", (namespace,)).fetchone()[0])
        assert policy["enabled"] and policy["epoch"] > 0 and str(case) not in json.dumps(policy)
        profiles = list((case / "home/capture/profiles").glob("*.json"))
        assert len(profiles) == 1 and profiles[0].stat().st_mode & 0o777 == 0o600
        assert str(source) not in profiles[0].read_text(), "functional record retained an exact transcript path"
        dispatch(case, namespace, session, source, "prompt", "turn_synthetic")
        write_source(source, final)
        dispatch(case, namespace, session, source, "stop", "turn_synthetic")
        bodies = [json.loads(row[0]) for row in database.execute("SELECT body FROM outbox ORDER BY revision")]
        observed_ids = [record["source_record_id"] for body in bodies for record in body["usage"].get("records", [])]
        statuses = [body["usage"]["status"] for body in bodies]
        observe({"namespace": namespace, "case": "normal-numeric-acceptance", "status": statuses,
                 "request_ids": observed_ids, "expected_request_ids": [new_id],
                 "timing_segments_preserved": bool(bodies) and bool(bodies[0]["segments"])})
        assert bodies and bodies[0]["segments"], "normal numeric path discarded legitimate timing"
        for raw, in database.execute("SELECT data FROM sessions UNION ALL SELECT body FROM outbox"):
            assert "PRIVATE_" not in raw and str(case) not in raw
        if observed_ids != [new_id] or "PARTIAL" not in statuses:
            failures.append(namespace + ": normal guarded route did not capture the new request delta")
        database.close()

    assert not failures, "; ".join(failures)
    print(json.dumps({"source_fixture_only": True, "both_namespaces_numeric_normal_route": True,
                      "real_harness_provider_or_release_qualification": False}))


if __name__ == "__main__":
    main()
