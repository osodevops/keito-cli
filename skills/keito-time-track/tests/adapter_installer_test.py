#!/usr/bin/env python3
"""Thin adapters/installers: temporary HOME and fake CLI only; no account/network."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import time
import unittest
from unittest import mock
import importlib.util
from types import SimpleNamespace
import sys
sys.dont_write_bytecode = True

SKILL = Path(__file__).resolve().parents[1]
EVENTS = {"session-start.sh": "session-start", "prompt.sh": "prompt", "stop.sh": "stop",
          "session-end.sh": "stop", "interrupt.sh": "interrupt", "session-finalize.sh": "session-end"}


class AdapterInstallerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="keito adapter '")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "home with spaces"
        self.home.mkdir()
        self.calls = self.root / "calls.jsonl"
        self.bin = self.root / "keito fake"
        self.bin.write_text("""#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
if sys.argv[1:] == ['capture', '--help']:
    sys.exit(0 if os.environ.get('FAKE_OLD_CLI') != '1' else 2)
record = {'args': sys.argv[1:], 'payload': sys.stdin.read() if sys.argv[1:3] == ['capture', 'hook'] else ''}
if 'setup' in sys.argv:
    record['mapping_at_setup'] = json.loads((Path.cwd() / '.keito/config.yml').read_text())
with open(os.environ['FAKE_CALLS'], 'a') as stream:
    stream.write(json.dumps(record) + '\\n')
if os.environ.get('FAKE_PICKER') == '1':
    if 'auth' in sys.argv:
        print(json.dumps({'authenticated': True, 'api_key_valid': True, 'account_id': 'co_test'}))
    elif 'clients' in sys.argv:
        print(json.dumps([{'id': 'c1', 'name': 'Synthetic client'}]))
    elif 'tasks' in sys.argv:
        print(json.dumps([{'id': 't1', 'name': 'Development'}]))
    elif 'projects' in sys.argv:
        print(json.dumps([{'id': 'p1', 'name': 'Synthetic project'}]))
    else:
        print('{}')
    sys.exit(0)
print('PRIVATE_CLI_OUTPUT_CANARY')
print('PRIVATE_CLI_ERROR_CANARY', file=sys.stderr)
time.sleep(float(os.environ.get('FAKE_SLEEP', '0')))
sys.exit(int(os.environ.get('FAKE_EXIT', '0')))
""")
        self.bin.chmod(0o700)
        self.env = {**os.environ, "HOME": str(self.home), "KEITO_CLI_BIN": str(self.bin),
                    "KEITO_AGENT_TYPE": "codex", "KEITO_HARNESS_VERSION": "0.167.0",
                    "FAKE_CALLS": str(self.calls)}
        for key in ("CLAUDE_PLUGIN_ROOT", "KEITO_SKILL_HOME", "KEITO_API_KEY", "KEITO_ACCOUNT_ID"):
            self.env.pop(key, None)
        # Warm only the fake Python executable before measuring adapter latency.
        subprocess.run([str(self.bin), "capture", "--help"], env=self.env, check=True, timeout=5)

    def run_script(self, path, *args, payload="", env=None):
        return subprocess.run(["bash", str(SKILL / path), *args], input=payload,
                              capture_output=True, text=True, env=env or self.env,
                              cwd=self.root, timeout=5)

    def recorded(self):
        return [json.loads(line) for line in self.calls.read_text().splitlines()] if self.calls.exists() else []

    def config_path(self, agent):
        return self.home / (".claude/settings.json" if agent == "claude-code" else ".codex/hooks.json")

    def install(self, agent="codex", *args):
        return self.run_script(f"installers/install-{agent}.sh", "--harness-version", "0.167.0", *args)

    def uninstall(self, agent="codex"):
        return self.run_script("installers/uninstall.sh", "--agent", agent)

    def read_config(self, agent="codex"):
        return json.loads(self.config_path(agent).read_text())

    def test_every_boundary_delegates_once_without_accounting_or_output(self):
        payload = json.dumps({"session_id": "s-exact", "turn_id": "turn-1", "cwd": str(self.root),
                              "prompt": "PRIVATE_PROMPT_CANARY", "transcript_path": "/private/canary"})
        for script, event in EVENTS.items():
            with self.subTest(script=script):
                result = self.run_script("hooks/" + script, payload=payload)
                self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))
                self.assertEqual(self.recorded()[-1], {"args": ["capture", "hook", "--agent", "codex",
                    "--event", event, "--harness-version", "0.167.0"], "payload": payload})
        self.assertEqual(len(self.recorded()), len(EVENTS))
        self.assertFalse((self.home / ".keito").exists(), "Adapters must not persist raw/session state")

    def test_runtime_failure_and_missing_cli_are_fail_open_and_silent(self):
        env = {**self.env, "FAKE_EXIT": "9"}
        for binary in (str(self.bin), "/missing"):
            result = self.run_script("hooks/stop.sh", payload="{}", env={**env, "KEITO_CLI_BIN": binary})
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))

    def test_stalled_core_has_one_second_fail_open_ceiling(self):
        started = time.monotonic()
        result = self.run_script("hooks/stop.sh", payload="{}", env={**self.env, "FAKE_SLEEP": "3"})
        self.assertLess(time.monotonic() - started, 1.6)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))

    def test_unknown_agent_does_not_invent_identity(self):
        result = self.run_script("hooks/prompt.sh", payload="{}", env={**self.env, "KEITO_AGENT_TYPE": "other"})
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.recorded(), [])

    def test_install_requires_explicit_targets(self):
        result = self.run_script("installers/install.sh")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.home / ".codex").exists())
        self.assertFalse((self.home / ".claude").exists())

    def test_selected_install_idempotent_preserves_mixed_registrations(self):
        for agent in ("claude-code", "codex"):
            with self.subTest(agent=agent):
                config = self.config_path(agent)
                config.parent.mkdir(exist_ok=True)
                original = {"unrelated": {"enabled": True}, "hooks": {"Stop": [{"matcher": "keep",
                    "hooks": [{"type": "command", "command": "printf untouched"}]}]}}
                config.write_text(json.dumps(original))
                first = self.install(agent)
                self.assertEqual(first.returncode, 0, first.stderr)
                once = config.read_bytes()
                second = self.install(agent)
                self.assertEqual(second.returncode, 0, second.stderr)
                self.assertEqual(config.read_bytes(), once)
                current = self.read_config(agent)
                self.assertEqual(current["unrelated"], original["unrelated"])
                self.assertEqual(current["hooks"]["Stop"][0], original["hooks"]["Stop"][0])
                expected_events = {"SessionStart", "UserPromptSubmit", "Stop", "SessionEnd",
                                   "StopFailure" if agent == "claude-code" else "Interrupt"}
                self.assertEqual(set(current["hooks"]), expected_events)
                for event in expected_events:
                    owned = [h for group in current["hooks"][event] for h in group["hooks"]
                             if "dispatch.sh" in h.get("command", "")]
                    self.assertEqual(len(owned), 1)
                    self.assertLessEqual(owned[0]["timeout"], 1)
                    self.assertIn("0.167.0", shlex.split(owned[0]["command"]))
                receipt = self.home / (".claude" if agent == "claude-code" else ".codex") / "skills/keito-time-track/capture-install.json"
                state = json.loads(receipt.read_text())
                self.assertEqual((state["trusted"], state["delivery_verified"], state["harness_version"]),
                                 ("unknown", False, "0.167.0"))
                self.assertEqual(config.stat().st_mode & 0o777, 0o600)

    def test_selected_target_leaves_other_harness_and_codex_trust_untouched(self):
        codex = self.home / ".codex"
        codex.mkdir()
        config = codex / "config.toml"
        trust = codex / "hooks-trust.json"
        config.write_text('[features]\nhooks = false\n[unrelated]\nvalue = "keep"\n')
        trust.write_text('{"trusted":false}')
        before = (config.read_bytes(), trust.read_bytes())
        result = self.install("codex")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((config.read_bytes(), trust.read_bytes()), before)
        self.assertFalse((self.home / ".claude").exists())

    def test_uninstall_preserves_unrelated_hook_inside_owned_group(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        current = self.read_config()
        current["hooks"]["Stop"][-1]["hooks"].append({"type": "command", "command": "echo keep"})
        current["other"] = ["preserve"]
        config.write_text(json.dumps(current))
        result = self.uninstall()
        self.assertEqual(result.returncode, 0, result.stderr)
        after = self.read_config()
        self.assertEqual(after["hooks"]["Stop"][-1]["hooks"], [{"type": "command", "command": "echo keep"}])
        self.assertEqual(after["other"], ["preserve"])
        self.assertNotIn("dispatch.sh", config.read_text())
        self.assertFalse((self.home / ".codex/skills/keito-time-track").exists())
        self.assertEqual(self.uninstall().returncode, 0)

    def test_malformed_config_preserves_original_and_existing_install(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        config.write_text('{"hooks":')
        skill = self.home / ".codex/skills/keito-time-track/SKILL.md"
        before = (config.read_bytes(), skill.read_bytes())
        for result in (self.install(), self.uninstall()):
            self.assertNotEqual(result.returncode, 0)
        self.assertEqual((config.read_bytes(), skill.read_bytes()), before)

    def test_wrong_shaped_hooks_fail_before_writing(self):
        for malformed in ([], {"hooks": []}, {"hooks": {"Stop": {}}}, {"hooks": {"Stop": [{"hooks": "bad"}]}}):
            config = self.config_path("codex")
            config.parent.mkdir(exist_ok=True)
            config.write_text(json.dumps(malformed))
            before = config.read_bytes()
            self.assertNotEqual(self.install().returncode, 0)
            self.assertEqual(config.read_bytes(), before)
            self.assertFalse((self.home / ".codex/skills/keito-time-track").exists())

    def test_legacy_plugin_needs_confirmation_preserves_other_plugin(self):
        config = self.config_path("claude-code")
        config.parent.mkdir()
        original = {"enabledPlugins": {"keito-time-track@keito": True, "other@market": True}}
        config.write_text(json.dumps(original))
        before = config.read_bytes()
        self.assertNotEqual(self.install("claude-code").returncode, 0)
        self.assertEqual(config.read_bytes(), before)
        result = self.install("claude-code", "--confirm-migration")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read_config("claude-code")["enabledPlugins"],
                         {"keito-time-track@keito": False, "other@market": True})

    def test_legacy_direct_migration_preserves_unrelated_sibling(self):
        config = self.config_path("codex")
        config.parent.mkdir()
        dest = self.home / ".codex/skills/keito-time-track"
        original = {"hooks": {"Stop": [{"hooks": [{"type": "command", "command":
            "KEITO_AGENT_TYPE=codex " + shlex.quote(str(dest / "hooks/session-end.sh"))},
            {"type": "command", "command": "echo unrelated"}]}]}}
        config.write_text(json.dumps(original))
        self.assertNotEqual(self.install().returncode, 0)
        result = self.install("codex", "--confirm-migration")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read_config()["hooks"]["Stop"][0]["hooks"], [{"type": "command", "command": "echo unrelated"}])

    def test_symlink_settings_cannot_overwrite_external_file(self):
        config = self.config_path("codex")
        config.parent.mkdir()
        external = self.root / "external-settings.json"
        external.write_text('{}')
        config.symlink_to(external)
        self.assertNotEqual(self.install().returncode, 0)
        self.assertTrue(config.is_symlink())
        self.assertEqual(external.read_text(), '{}')

    def test_old_runtime_cannot_install_new_adapters(self):
        result = self.run_script("installers/install-codex.sh", "--harness-version", "0.167.0",
                                 env={**self.env, "FAKE_OLD_CLI": "1"})
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.config_path("codex").exists())

    def test_installed_commands_preserve_quoted_paths_and_exact_harness_identity(self):
        result = self.install("claude-code")
        self.assertEqual(result.returncode, 0, result.stderr)
        command = self.read_config("claude-code")["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
        result = subprocess.run(["bash", "-c", command], input='{"session_id":"exact"}', text=True,
                                capture_output=True, env=self.env, timeout=3)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))
        self.assertEqual(self.recorded()[-1], {"args": ["capture", "hook", "--agent", "claude-code",
            "--event", "prompt", "--harness-version", "0.167.0"], "payload": '{"session_id":"exact"}'})

    def test_failed_atomic_config_replacement_restores_previous_payload(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        receipt = self.home / ".codex/skills/keito-time-track/capture-install.json"
        before = (config.read_bytes(), receipt.read_bytes())
        spec = importlib.util.spec_from_file_location("keito_installer_under_test", SKILL / "installers/manage-hooks.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        replace = module.os.replace
        def fail_config(source, destination):
            if Path(destination) == config:
                raise OSError("synthetic interrupted replacement")
            return replace(source, destination)
        args = SimpleNamespace(confirm_migration=False, harness_version="0.167.1")
        with mock.patch.dict(os.environ, self.env, clear=True), mock.patch.object(module.os, "replace", side_effect=fail_config):
            with self.assertRaises(OSError):
                module.apply("codex", "install", args, str(self.bin))
        self.assertEqual((config.read_bytes(), receipt.read_bytes()), before)
        self.assertFalse(list(config.parent.glob(".hooks.json.keito-*")))

    def test_modified_skill_payload_is_not_deleted_on_uninstall(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        extra = self.home / ".codex/skills/keito-time-track/assets/capture-install.json"
        extra.write_text("preserve local edits")
        before = config.read_bytes()
        result = self.uninstall()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(config.read_bytes(), before)
        self.assertEqual(extra.read_text(), "preserve local edits")

    def test_root_plugin_manifest_has_all_boundaries_and_no_network_or_legacy_handler(self):
        manifest = SKILL / "hooks/claude-plugin.json"
        data = json.loads(manifest.read_text())
        root_manifest = SKILL.parent / "hooks/hooks.json"
        if root_manifest.exists():
            self.assertEqual(data, json.loads(root_manifest.read_text()))
        expected = {"SessionStart": "session-start", "UserPromptSubmit": "prompt", "Stop": "stop",
                    "StopFailure": "interrupt", "SessionEnd": "session-end"}
        self.assertEqual(set(data["hooks"]), set(expected))
        for event, lifecycle in expected.items():
            hook = data["hooks"][event][0]["hooks"][0]
            self.assertEqual(hook["timeout"], 1)
            env = {**self.env, "CLAUDE_PLUGIN_ROOT": str(SKILL.parent)}
            result = subprocess.run(["bash", "-c", hook["command"]], input="{}", capture_output=True,
                                    text=True, env=env, timeout=3)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))
            self.assertEqual(self.recorded()[-1]["args"], ["capture", "hook", "--agent", "claude-code",
                "--event", lifecycle, "--harness-version", "0.167.0"])

    def test_setup_picker_scopes_existing_tasks_and_approves_final_hint_only_after_confirmation(self):
        subprocess.run(["git", "-c", "init.templateDir=", "init", "--quiet", str(self.root)], check=True, env=self.env)
        result = self.run_script("scripts/setup-wizard.sh", "--agent", "codex", payload="1\n1\n1\ny\n",
                                 env={**self.env, "FAKE_PICKER": "1"})
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.recorded()
        self.assertIn(["--json", "--workspace", "co_test", "projects", "tasks", "p1"], [c["args"] for c in calls])
        setup = [c for c in calls if "setup" in c["args"]]
        self.assertEqual(len(setup), 1)
        self.assertEqual(setup[0]["args"], ["--workspace", "co_test", "capture", "setup", "--agent", "codex",
                         "--project", "p1", "--task", "t1", "--confirm"])
        self.assertEqual(setup[0]["mapping_at_setup"]["agent_tracking"],
                         {"enabled": True, "source": "agent", "redact_notes": True})
        self.assertFalse(any("create" in c["args"] for c in calls))
        self.assertEqual((self.root / ".keito/config.yml").stat().st_mode & 0o777, 0o600)

    def test_setup_cancel_or_missing_targets_cannot_approve_mapping(self):
        subprocess.run(["git", "-c", "init.templateDir=", "init", "--quiet", str(self.root)], check=True, env=self.env)
        self.assertNotEqual(self.run_script("scripts/setup-wizard.sh").returncode, 0)
        result = self.run_script("scripts/setup-wizard.sh", "--agent", "codex", payload="1\n1\n1\nn\n",
                                 env={**self.env, "FAKE_PICKER": "1"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / ".keito/config.yml").exists())
        self.assertFalse(any("setup" in c["args"] for c in self.recorded()))

    def test_api_incompatible_version_is_rejected_without_silent_truncation(self):
        result = self.install("codex", "--harness-version", "0.167.0+build")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.config_path("codex").exists())
        result = self.run_script("hooks/stop.sh", payload="{}", env={**self.env, "KEITO_HARNESS_VERSION": "0.167.0+build"})
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))
        self.assertEqual(self.recorded(), [])

    def test_management_scripts_delegate_without_legacy_state(self):
        for path, args, expected in [
            ("scripts/status.sh", [], ["capture", "status"]),
            ("scripts/pause-resume.sh", ["pause"], ["capture", "pause", "--repo"]),
            ("scripts/pause-resume.sh", ["resume"], ["capture", "resume", "--repo"]),
            ("scripts/disable.sh", [], ["capture", "disable", "--repo"]),
        ]:
            with self.subTest(path=path, args=args):
                result = self.run_script(path, *args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.recorded()[-1]["args"], expected)


if __name__ == "__main__":
    unittest.main(verbosity=2)
