#!/usr/bin/env python3
"""Owned actual normal-route negatives; no real harness/provider/release claim."""
from pathlib import Path
import json
import os
import shutil
import uuid
import sys
sys.dont_write_bytecode = True
import guarded_usage_acceptance as qa


def source_records(namespace, session, version):
    if namespace == "codex":
        initial = [{"type": "session_meta", "payload": {"id": session, "session_id": session, "cli_version": version}}]
        final = initial + [{"type": "turn_context", "payload": {"turn_id": "turn_synthetic", "model": "gpt-6.1-sol"}},
                           {"type": "token_usage_record", "payload": {"session_id": session, "thread_id": session,
                            "turn_id": "turn_synthetic", "root_turn_id": "turn_synthetic", "response_id": "resp_after_prompt",
                            "usage": {"input_tokens": 101, "output_tokens": 5}}}]
    else:
        initial = []
        final = [{"type": "assistant", "sessionId": session, "message": {"id": "msg_after_prompt",
                  "model": "claude-opus-5-5", "stop_reason": "end_turn", "usage": {"input_tokens": 2, "output_tokens": 5}}}]
    return initial, final


def setup(namespace, version, filename):
    case, database = qa.make_case(namespace)
    image = case / "native-pinned-owned"
    shutil.copyfile(qa.NATIVES / filename, image)
    image.chmod(0o700)
    session = str(uuid.uuid4())
    source = (case / "home/.codex/sessions/2026/11/08" / ("rollout-owned-" + session + ".jsonl") if namespace == "codex"
              else case / "home/.claude/projects/owned" / (session + ".jsonl"))
    initial, final = source_records(namespace, session, version)
    qa.write_source(source, initial)
    result = qa.cli(case, ["capture", "usage", "enable", "--repo", "--agent", namespace,
                          "--profile-home", str(case / "home"), "--harness-executable", str(image),
                          "--native-executable", str(image), "--confirm", "--json"])
    qa.observe({"namespace": namespace, "case": "negative-explicit-enable", "exit": result.returncode})
    assert result.returncode == 0
    return case, database, image, session, source, final


def assert_time_unknown(case, database, namespace, label, expected):
    bodies = [json.loads(row[0]) for row in database.execute("SELECT body FROM outbox ORDER BY revision")]
    assert bodies and bodies[0]["segments"], "negative discarded legitimate timing"
    assert all(not body["usage"].get("records") for body in bodies), "negative accepted uncertain numeric evidence"
    result = qa.cli(case, ["capture", "usage", "status", "--repo", "--agent", namespace, "--json"])
    assert result.returncode == 0
    status = json.loads(result.stdout)
    qa.observe({"namespace": namespace, "case": label, "reason": status["reason"],
                "enabled": status["enabled"], "timing_preserved": True, "numeric_records": 0})
    assert status["reason"] == expected, (namespace, label, status)
    for raw, in database.execute("SELECT data FROM sessions UNION ALL SELECT body FROM outbox"):
        assert "PRIVATE_" not in raw and str(case) not in raw


MATRIX = {
    "profile-record-changed": "PROFILE_CHANGED", "managed-root-recreated": "PROFILE_CHANGED",
    "native-image-changed": "HARNESS_CHANGED", "registration-changed": "REGISTRATION_CHANGED",
    "generation-changed": "REGISTRATION_CHANGED", "generation-receipt-changed": "REGISTRATION_CHANGED",
    "source-replaced": "SOURCE_CHANGED", "source-symlink": "SOURCE_PARSE_UNAVAILABLE",
    "source-fifo": "SOURCE_PARSE_UNAVAILABLE", "source-hardlink": "SOURCE_PARSE_UNAVAILABLE",
    "source-directory": "SOURCE_PARSE_UNAVAILABLE", "source-wrong-session": "SOURCE_PARSE_UNAVAILABLE",
    "path-missing": "ACTIVE_SOURCE_UNAVAILABLE", "path-null": "ACTIVE_SOURCE_UNAVAILABLE",
    "path-wrong-namespace": "ACTIVE_SOURCE_UNAVAILABLE", "path-wrong-session": "ACTIVE_SOURCE_UNAVAILABLE",
    "numeric-revoked": "NUMERIC_CAPTURE_DISABLED", "metadata-paused": "NUMERIC_CAPTURE_DISABLED",
}
for namespace, version, filename in [("codex", "0.167.0", "codex-inert-source-only"),
                                      ("claude-code", "2.1.319", "claude-inert-source-only")]:
    for label, expected in MATRIX.items():
        case, database, image, session, source, final = setup(namespace, version, filename)
        qa.dispatch(case, namespace, session, source, "prompt", "turn_synthetic", program_override=image)
        qa.write_source(source, final)
        options = {}
        profile_path = next((case / "home/capture/profiles").glob("*.json"))
        profile = json.loads(profile_path.read_text())
        if label == "profile-record-changed":
            profile_path.write_text(profile_path.read_text() + " ")
        elif label == "managed-root-recreated":
            managed = Path(profile["managed_root"])
            managed.rename(managed.with_name(managed.name + "-previous"))
            managed.mkdir(mode=0o700)
        elif label == "native-image-changed":
            os.utime(image, ns=(image.stat().st_atime_ns, image.stat().st_mtime_ns + 1_000_000_000))
        elif label == "registration-changed":
            config = case / ("home/.codex/hooks.json" if namespace == "codex" else "home/.claude/settings.json")
            config.write_text(config.read_text() + " ")
        elif label == "generation-changed":
            Path(profile["generation_root"]).chmod(0o500)
        elif label == "generation-receipt-changed":
            receipt = Path(profile["generation_root"]) / "capture-install.json"
            os.utime(receipt, ns=(receipt.stat().st_atime_ns, receipt.stat().st_mtime_ns + 1_000_000_000))
        elif label.startswith("source-") and label != "source-wrong-session":
            previous = source.with_suffix(".previous")
            source.rename(previous)
            if label == "source-replaced":
                source.write_text("PRIVATE_REPLACED_INVALID_SOURCE")
            elif label == "source-symlink":
                source.symlink_to(previous)
            elif label == "source-fifo":
                os.mkfifo(source, 0o600)
            elif label == "source-hardlink":
                os.link(previous, source)
            elif label == "source-directory":
                source.mkdir(mode=0o700)
        elif label == "source-wrong-session":
            source.write_text(source.read_text().replace(session, str(uuid.uuid4())))
        elif label == "path-null":
            options["overrides"] = {"transcript_path": None}
        elif label == "path-missing":
            options["remove_fields"] = ["transcript_path"]
        elif label == "path-wrong-namespace":
            options["overrides"] = {"transcript_path": str(case / "home" / (".claude/projects/owned" if namespace == "codex" else ".codex/sessions/2026/11/08") / source.name)}
        elif label == "path-wrong-session":
            options["overrides"] = {"transcript_path": str(source.with_name(str(uuid.uuid4()) + ".jsonl"))}
        elif label == "numeric-revoked":
            result = qa.cli(case, ["capture", "usage", "disable", "--repo", "--agent", namespace, "--json"])
            assert result.returncode == 0
        elif label == "metadata-paused":
            result = qa.cli(case, ["capture", "pause", "--repo", "--json"])
            assert result.returncode == 0
        qa.dispatch(case, namespace, session, source, "stop", "turn_synthetic", program_override=image, **options)
        assert_time_unknown(case, database, namespace, label, expected)
        database.close()
    # The wrong native-image chain is entirely run-owned. Eight added native
    # ancestors ensure no observation reaches the QA runner or operator process.
    case, database, image, session, source, final = setup(namespace, version, filename)
    wrong = case / "native-wrong-owned"
    shutil.copyfile(qa.NATIVES / filename, wrong)
    wrong.chmod(0o700)
    qa.dispatch(case, namespace, session, source, "prompt", "turn_synthetic", program_override=wrong, chain_depth=8)
    status = json.loads(qa.cli(case, ["capture", "usage", "status", "--repo", "--agent", namespace, "--json"]).stdout)
    assert status["reason"] == "HARNESS_ORIGIN_UNAVAILABLE", status
    qa.observe({"namespace": namespace, "case": "wrong-owned-native-image-bounded-chain", "reason": status["reason"], "owned_chain_depth": 8})
    qa.write_source(source, final)
    qa.dispatch(case, namespace, session, source, "stop", "turn_synthetic", program_override=image)
    assert_time_unknown(case, database, namespace, "terminal-cannot-repair-wrong-origin", "MISSING_QUALIFIED_PROMPT")
    database.close()
print(json.dumps({"source_fixture_only": True, "normal_guarded_negative_cases": 2 * (len(MATRIX) + 1),
                  "real_harness_provider_or_release_qualification": False}))
