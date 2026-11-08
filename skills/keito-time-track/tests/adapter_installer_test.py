#!/usr/bin/env python3
"""Thin adapters/installers: temporary HOME and fake CLI only; no account/network."""
import base64
import json
import os
from pathlib import Path
import shlex
import signal
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
if os.environ.get('FAKE_EXPECT_PRELAUNCH'):
    import fcntl
    pending = Path(os.environ['KEITO_CAPTURE_HOME']) / 'capture-invocations' / (os.environ.get('KEITO_CAPTURE_INVOCATION_NONCE', '') + '.pending')
    record['prelaunch_valid'] = pending.is_file() and pending.read_bytes() == b'KEITO_CAPTURE_PENDING_V1\\n'
    record['prelaunch_locked'] = False
    if pending.is_file():
        with pending.open('rb') as marker:
            try:
                fcntl.flock(marker, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                record['prelaunch_locked'] = True
if os.environ.get('FAKE_CORE_PID'):
    Path(os.environ['FAKE_CORE_PID']).write_text(str(os.getpid()) + ' ' + str(os.getppid()))
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
    if 'setup' in sys.argv:
        if os.environ.get('FAKE_SETUP_MARKER'):
            Path(os.environ['FAKE_SETUP_MARKER']).write_text('consent may be committed')
        time.sleep(float(os.environ.get('FAKE_SETUP_SLEEP', '0')))
        sys.exit(int(os.environ.get('FAKE_SETUP_EXIT', '0')))
    sys.exit(0)
print('PRIVATE_CLI_OUTPUT_CANARY')
print('PRIVATE_CLI_ERROR_CANARY', file=sys.stderr)
if os.environ.get('FAKE_IGNORE_TERM'):
    __import__('signal').signal(__import__('signal').SIGTERM, __import__('signal').SIG_IGN)
    if os.environ.get('FAKE_TERM_MARKER'):
        Path(os.environ['FAKE_TERM_MARKER']).write_text('TERM ignored')
time.sleep(float(os.environ.get('FAKE_SLEEP', '0')))
exit_code = int(os.environ.get('FAKE_EXIT', '0'))
if (exit_code == 0 or os.environ.get('FAKE_ACK_ALWAYS') == '1') and os.environ.get('FAKE_NO_ACK') != '1' and os.environ.get('KEITO_CAPTURE_ACK_FD'):
    os.write(int(os.environ['KEITO_CAPTURE_ACK_FD']), os.environ.get('FAKE_ACK_BYTES', 'KEITO_CAPTURE_DURABLE_V1\\n').encode())
sys.exit(exit_code)
""")
        self.bin.chmod(0o700)
        self.env = {**os.environ, "HOME": str(self.home), "KEITO_CLI_BIN": str(self.bin),
                    "KEITO_AGENT_TYPE": "codex", "KEITO_HARNESS_VERSION": "0.167.0",
                    "FAKE_CALLS": str(self.calls), "KEITO_CAPTURE_HOME": str((self.root / 'capture-state').resolve())}
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
        self.assertLess(time.monotonic() - started, 1.0)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))

    def test_durable_prelaunch_marker_precedes_core_and_only_exact_private_ack_clears_it(self):
        result = self.run_script('hooks/prompt.sh', payload='PRIVATE_PRELAUNCH_CANARY',
            env={**self.env, 'FAKE_EXPECT_PRELAUNCH': '1'})
        self.assertEqual((result.returncode,result.stdout,result.stderr),(0,'',''))
        call = self.recorded()[-1]
        self.assertTrue(call['prelaunch_valid'])
        self.assertTrue(call['prelaunch_locked'])
        directory = Path(self.env['KEITO_CAPTURE_HOME']) / 'capture-invocations'
        self.assertEqual(list(directory.iterdir()), [])
        for value in ('missing', 'KEITO_CAPTURE_DURABLE_V1\nextra', 'KEITO_CAPTURE_DURABLE_V1'):
            env = {**self.env, 'FAKE_NO_ACK': '1'} if value == 'missing' else {**self.env, 'FAKE_ACK_BYTES': value}
            result = self.run_script('hooks/stop.sh',payload='PRIVATE_PRELAUNCH_CANARY',env=env)
            self.assertEqual((result.returncode,result.stdout,result.stderr),(0,'',''))
        crashed = self.run_script('hooks/stop.sh',payload='PRIVATE_CRASH_CANARY',
            env={**self.env,'FAKE_EXIT':'9','FAKE_ACK_ALWAYS':'1'})
        self.assertEqual((crashed.returncode,crashed.stdout,crashed.stderr),(0,'',''))
        markers = list(directory.iterdir())
        self.assertEqual(len(markers),4)
        for marker in markers:
            self.assertRegex(marker.name,r'^[a-f0-9]{64}\.pending$')
            self.assertEqual(marker.read_bytes(),b'KEITO_CAPTURE_PENDING_V1\n')
            self.assertEqual(marker.stat().st_mode & 0o777,0o600)
        self.assertEqual(directory.stat().st_mode & 0o777,0o700)

    def test_guard_rejects_zero_length_storage_write_without_spinning(self):
        spec = importlib.util.spec_from_file_location('guard_zero_write',SKILL/'hooks/guard.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        directory = self.root/'zero-write-invocations'
        directory.mkdir(mode=0o700)
        fd = os.open(directory,os.O_RDONLY | os.O_DIRECTORY)
        try:
            with mock.patch.object(module.os,'write',side_effect=[0,AssertionError('A zero write must refuse before another write')]):
                with self.assertRaises(module.GuardRefused):
                    module.prepare(fd,'a'*64)
            self.assertEqual((directory/('a'*64+'.preparing')).read_bytes(),b'')
        finally:
            os.close(fd)

    def test_first_attempt_core_that_never_enters_handler_retains_failure_evidence(self):
        # No preflight/warmup of this executable. Delay is before its first
        # observable handler action, matching the retained cold-core failure.
        core = self.root / 'cold-never-entered'
        entered = self.root / 'handler-entered'
        core.write_text('#!/bin/sh\nsleep 2\nprintf entered > "$FAKE_ENTRY"\n')
        core.chmod(0o700)
        started = time.monotonic()
        result = self.run_script('hooks/stop.sh',payload='PRIVATE_COLD_CANARY',
            env={**self.env,'KEITO_CLI_BIN':str(core),'FAKE_ENTRY':str(entered)})
        self.assertLess(time.monotonic()-started,1.0)
        self.assertEqual((result.returncode,result.stdout,result.stderr),(0,'',''))
        self.assertFalse(entered.exists())
        directory = Path(self.env['KEITO_CAPTURE_HOME']) / 'capture-invocations'
        self.assertTrue(directory.is_dir(),'A prelaunch marker must exist even when the core never reaches its handler')
        markers = list(directory.iterdir())
        self.assertEqual(len(markers),1)
        self.assertEqual(markers[0].read_bytes(),b'KEITO_CAPTURE_PENDING_V1\n')
        import fcntl
        with markers[0].open('rb') as marker:
            fcntl.flock(marker,fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_guard_refuses_symlinked_capture_root_or_invocation_directory(self):
        external = self.root / 'external-capture'
        external.mkdir(mode=0o700)
        link = self.root / 'capture-link'
        link.symlink_to(external,target_is_directory=True)
        result = self.run_script('hooks/prompt.sh',payload='{}',env={**self.env,'KEITO_CAPTURE_HOME':str(link)})
        self.assertEqual((result.returncode,result.stdout,result.stderr),(0,'',''))
        self.assertEqual(self.recorded(),[])
        self.assertEqual(list(external.iterdir()),[])
        capture = Path(self.env['KEITO_CAPTURE_HOME'])
        capture.mkdir(mode=0o700)
        (capture / 'capture-invocations').symlink_to(external,target_is_directory=True)
        result = self.run_script('hooks/prompt.sh',payload='{}')
        self.assertEqual((result.returncode,result.stdout,result.stderr),(0,'',''))
        self.assertEqual(self.recorded(),[])
        self.assertEqual(list(external.iterdir()),[])

    def test_parallel_invocations_have_distinct_locked_markers_until_durable_ack(self):
        import fcntl
        env = {**self.env,'FAKE_SLEEP':'0.25','FAKE_EXPECT_PRELAUNCH':'1'}
        children = [subprocess.Popen(['bash',str(SKILL/'hooks/prompt.sh')],stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,cwd=self.root) for _ in range(2)]
        def cleanup():
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.communicate(timeout=2)
        self.addCleanup(cleanup)
        for child in children:
            child.stdin.write(b'PRIVATE_PARALLEL_CANARY'); child.stdin.close(); child.stdin=None
        directory = Path(self.env['KEITO_CAPTURE_HOME']) / 'capture-invocations'
        deadline = time.monotonic()+0.5
        markers = []
        while time.monotonic()<deadline:
            markers = list(directory.iterdir()) if directory.exists() else []
            if len(markers)==2: break
            time.sleep(0.005)
        self.assertEqual(len(markers),2)
        for marker in markers:
            with marker.open('rb') as stream:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(stream,fcntl.LOCK_EX | fcntl.LOCK_NB)
        for child in children:
            out,err=child.communicate(timeout=1)
            self.assertEqual((child.returncode,out,err),(0,b'',b''))
        self.assertEqual(list(directory.iterdir()),[])

    def test_missing_core_and_publication_lock_timeout_retain_classified_evidence(self):
        import fcntl
        capture = Path(self.env['KEITO_CAPTURE_HOME'])
        capture.mkdir(mode=0o700)
        lock = capture / 'capture-invocations-publication.lock'
        fd = os.open(lock,os.O_RDWR | os.O_CREAT,0o600)
        try:
            fcntl.flock(fd,fcntl.LOCK_EX | fcntl.LOCK_NB)
            started = time.monotonic()
            result = self.run_script('hooks/prompt.sh',payload='PRIVATE_LOCK_CANARY')
            self.assertLess(time.monotonic()-started,1.0)
            self.assertEqual((result.returncode,result.stdout,result.stderr),(0,'',''))
            self.assertEqual(self.recorded(),[])
            flags = list((capture/'capture-invocations').iterdir())
            self.assertEqual(len(flags),1)
            self.assertTrue(flags[0].name.endswith('.preparing'))
            self.assertEqual(flags[0].read_bytes(),b'KEITO_CAPTURE_PENDING_V1\n')
        finally:
            os.close(fd)
        result = self.run_script('hooks/prompt.sh',payload='PRIVATE_MISSING_CANARY',
            env={**self.env,'KEITO_CLI_BIN':'/missing-keito-private'})
        self.assertEqual((result.returncode,result.stdout,result.stderr),(0,'',''))
        self.assertEqual(self.recorded(),[])
        flags = list((capture/'capture-invocations').iterdir())
        self.assertEqual(len(flags),2)
        self.assertEqual(sum(p.name.endswith('.pending') for p in flags),1)
        self.assertTrue(all(p.read_bytes()==b'KEITO_CAPTURE_PENDING_V1\n' for p in flags))

    def test_sigkill_of_guard_releases_marker_lock_even_if_core_survives(self):
        import fcntl
        pids = self.root / 'core-and-guard-pids'
        child = subprocess.Popen(['bash',str(SKILL/'hooks/prompt.sh')],stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,stderr=subprocess.PIPE,env={**self.env,'FAKE_SLEEP':'3','FAKE_CORE_PID':str(pids)},cwd=self.root)
        child.stdin.write(b'PRIVATE_GUARD_KILL_CANARY'); child.stdin.close(); child.stdin=None
        core_pid = None
        try:
            deadline = time.monotonic()+0.55
            while not pids.exists() and time.monotonic()<deadline and child.poll() is None:
                time.sleep(0.005)
            self.assertTrue(pids.exists(),'The real guard must launch its child before this SIGKILL observation')
            core_pid,guard_pid = map(int,pids.read_text().split())
            os.kill(guard_pid,signal.SIGKILL)
            out,err = child.communicate(timeout=1)
            self.assertEqual((child.returncode,out,err),(0,b'',b''))
            os.kill(core_pid,0)
            flags = list((Path(self.env['KEITO_CAPTURE_HOME'])/'capture-invocations').iterdir())
            self.assertEqual(len(flags),1)
            self.assertEqual(flags[0].read_bytes(),b'KEITO_CAPTURE_PENDING_V1\n')
            with flags[0].open('rb') as flag:
                fcntl.flock(flag,fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            if core_pid is not None:
                try:
                    os.killpg(core_pid,signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=1)

    def test_real_signals_during_preparation_and_publication_retain_minimized_flags(self):
        wrapper = r'''
import os, pathlib, runpy, signal, stat, sys
script, core, boundary, signum = sys.argv[1:]
open_real, sync_real, rename_real = os.open, os.fsync, os.rename
phase = 'starting'
marker_fd = None
def stop_at(where):
    if boundary == where:
        os.kill(os.getpid(),int(signum))
def opened(path,*args,**kwargs):
    global marker_fd, phase
    fd = open_real(path,*args,**kwargs)
    if str(path).endswith('.preparing'):
        marker_fd = fd
        phase = 'preparing'
        stop_at('preparing_created')
    return fd
def synced(fd):
    result = sync_real(fd)
    if fd == marker_fd:
        stop_at('preparing_file_fsynced')
    elif phase == 'published' and stat.S_ISDIR(os.fstat(fd).st_mode):
        stop_at('pending_directory_fsynced')
    return result
def renamed(source,destination,*args,**kwargs):
    global phase
    result = rename_real(source,destination,*args,**kwargs)
    if str(destination).endswith('.pending'):
        phase = 'published'
        stop_at('pending_renamed')
    return result
os.open, os.fsync, os.rename = opened, synced, renamed
sys.argv = [script,core,'codex','prompt','0.167.0']
runpy.run_path(script,run_name='__main__')
'''
        import fcntl
        for boundary in ('preparing_created','preparing_file_fsynced','pending_renamed','pending_directory_fsynced'):
            for sig in (signal.SIGTERM,signal.SIGKILL):
                with self.subTest(boundary=boundary,signal=int(sig)):
                    capture = (self.root/('signal-'+boundary+'-'+str(int(sig)))).resolve()
                    result = subprocess.run([sys.executable,'-c',wrapper,str(SKILL/'hooks/guard.py'),
                        str(self.bin),boundary,str(int(sig))],input='PRIVATE_SIGNAL_CANARY',
                        env={**self.env,'KEITO_CAPTURE_HOME':str(capture)},capture_output=True,text=True,timeout=3)
                    self.assertEqual(result.returncode,0 if sig==signal.SIGTERM else -int(sig),result.stderr)
                    self.assertEqual((result.stdout,result.stderr),('',''))
                    flags = list((capture/'capture-invocations').iterdir())
                    self.assertEqual(len(flags),1)
                    self.assertRegex(flags[0].name,r'^[a-f0-9]{64}\.(preparing|pending)$')
                    self.assertTrue(b'KEITO_CAPTURE_PENDING_V1\n'.startswith(flags[0].read_bytes()))
                    self.assertNotIn(b'PRIVATE_SIGNAL_CANARY',flags[0].read_bytes())
                    with flags[0].open('rb') as flag:
                        fcntl.flock(flag,fcntl.LOCK_EX | fcntl.LOCK_NB)
                    self.assertEqual(self.recorded(),[],'No heavy core may start before durable pending publication')

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
            if Path(destination).resolve() == config.resolve():
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

    def test_unrelated_commands_mentioning_adapter_as_data_survive_install_and_uninstall(self):
        config = self.config_path("codex")
        config.parent.mkdir()
        script = str(self.home / ".codex/skills/keito-time-track/hooks/dispatch.sh")
        commands = ["printf '%s' " + shlex.quote(script), "audit " + shlex.quote(script),
                    "bash -c " + shlex.quote("echo " + script),
                    shlex.quote(script) + " --agent codex --event stop --harness-version 0.167.0 && echo other"]
        hooks = [{"type": "command", "command": command} for command in commands]
        config.write_text(json.dumps({"hooks": {"Stop": [{"hooks": hooks}]}}))
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        for original in hooks:
            self.assertIn(original, [h for g in self.read_config()["hooks"]["Stop"] for h in g["hooks"]])
        self.assertEqual(self.uninstall().returncode, 0)
        self.assertEqual(self.read_config()["hooks"]["Stop"][0]["hooks"], hooks)

    def test_symlinked_harness_or_skills_parents_refuse_install_and_uninstall(self):
        for parent_name in (".codex", ".codex/skills", ".claude", ".claude/skills"):
            with self.subTest(parent=parent_name):
                case_home = self.root / ("case-" + parent_name.replace('/', '-'))
                case_home.mkdir()
                target = self.root / ("outside-" + parent_name.replace('/', '-'))
                target.mkdir()
                marker = target / "preserve.txt"
                marker.write_text("outside must remain intact")
                link = case_home / parent_name
                link.parent.mkdir(parents=True, exist_ok=True)
                link.symlink_to(target, target_is_directory=True)
                agent = "codex" if parent_name.startswith(".codex") else "claude-code"
                env = {**self.env, "HOME": str(case_home)}
                before = sorted(str(p.relative_to(target)) for p in target.rglob('*'))
                for action in ("install", "uninstall"):
                    script = f"installers/install-{agent}.sh" if action == "install" else "installers/uninstall.sh"
                    args = ["--harness-version", "0.167.0"] if action == "install" else ["--agent", agent]
                    result = self.run_script(script, *args, env=env)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertNotIn("Traceback", result.stderr)
                    self.assertEqual(sorted(str(p.relative_to(target)) for p in target.rglob('*')), before)
                    self.assertEqual(marker.read_text(), "outside must remain intact")

    def test_malformed_receipt_shapes_fail_with_sanitized_recovery_and_no_mutations(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        receipt = self.home / ".codex/skills/keito-time-track/capture-install.json"
        before = config.read_bytes()
        for content in (b'[]', b'null', b'"text"', b'\xff', b'{"files":[]}', b'[' * 1200 + b'0' + b']' * 1200):
            with self.subTest(content=repr(content)):
                receipt.write_bytes(content)
                result = self.uninstall()
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("Traceback", result.stderr)
                self.assertNotIn(str(self.home), result.stderr)
                self.assertIn("review", result.stderr.lower())
                self.assertEqual((config.read_bytes(), receipt.read_bytes()), (before, content))

    def test_stalled_term_ignoring_core_returns_before_installed_host_deadline(self):
        self.assertEqual(self.install().returncode, 0)
        command = self.read_config()["hooks"]["Stop"][0]["hooks"][0]["command"]
        marker = self.root / "term-handler-armed"
        started = time.monotonic()
        result = subprocess.run(["bash", "-c", command], input="{}", text=True, capture_output=True,
                                env={**self.env, "FAKE_SLEEP": "3", "FAKE_IGNORE_TERM": "1", "FAKE_TERM_MARKER": str(marker)}, timeout=1.0)
        self.assertTrue(marker.exists(), "Fixture must install its TERM-ignore handler")
        self.assertLess(time.monotonic() - started, 1.0)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))

    def test_failed_setup_restores_previous_hint_even_after_possible_consent_commit(self):
        subprocess.run(["git", "-c", "init.templateDir=", "init", "--quiet", str(self.root)], check=True, env=self.env)
        hint = self.root / ".keito/config.yml"
        hint.parent.mkdir()
        old = b'{"workspace_id":"old","project_id":"oldproject","task_id":"oldtask","agent_tracking":{"enabled":false}}'
        hint.write_bytes(old)
        marker = self.root / "consent-marker"
        result = self.run_script("scripts/setup-wizard.sh", "--agent", "codex", payload="1\n1\n1\ny\n",
            env={**self.env, "FAKE_PICKER": "1", "FAKE_SETUP_EXIT": "9", "FAKE_SETUP_MARKER": str(marker)})
        self.assertEqual(result.returncode, 9)
        self.assertTrue(marker.exists())
        self.assertEqual(hint.read_bytes(), old)
        self.assertIn("existing", result.stdout.lower())
        self.assertIn("status", result.stderr.lower())

    def test_install_survives_real_signals_at_each_publication_boundary_and_recovers(self):
        self.assertEqual(self.install().returncode, 0)
        child = r"""import importlib.util,os,pathlib,signal,sys
from types import SimpleNamespace
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('installer',sys.argv[1]);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
real=m.os.replace
boundary=sys.argv[3];sig=int(sys.argv[4])
def replace(src,dst):
    src,dst=pathlib.Path(src),pathlib.Path(dst)
    hit=(boundary=='payload-backup' and src.name=='keito-time-track') or (boundary=='config-before' and dst.name=='hooks.json') or (boundary=='config-after' and dst.name=='hooks.json') or (boundary=='generation' and dst.parent.name=='.keito-capture-generations')
    if hit and boundary=='config-before':os.kill(os.getpid(),sig)
    real(src,dst)
    if hit and boundary!='config-before':os.kill(os.getpid(),sig)
m.os.replace=replace
m.apply('codex','install',SimpleNamespace(confirm_migration=False,harness_version='0.167.1-'+boundary+'-'+str(sig)),sys.argv[2])
"""
        for boundary in ("payload-backup", "config-before", "config-after", "generation"):
            for sig in (signal.SIGTERM, signal.SIGKILL):
                with self.subTest(boundary=boundary, signal=sig):
                    self.assertEqual(self.install().returncode, 0)
                    result = subprocess.run([sys.executable, "-c", child, str(SKILL / "installers/manage-hooks.py"),
                                             str(self.bin), boundary, str(int(sig))], env=self.env, capture_output=True, text=True, timeout=15)
                    self.assertIn(result.returncode, (-sig, 128 + sig), result.stderr)
                    for groups in self.read_config()["hooks"].values():
                        for group in groups:
                            for hook in group["hooks"]:
                                argv = shlex.split(hook["command"])
                                executable = next(token for token in argv if token.endswith('/hooks/dispatch.sh'))
                                self.assertTrue(Path(executable).is_file(), "Registered payload must remain available after process death")
                    recovery = self.install()
                    self.assertEqual(recovery.returncode, 0, recovery.stderr)
                    self.assertTrue((self.home / ".codex/skills/keito-time-track/SKILL.md").is_file())

    def test_mapping_signal_failure_restores_or_recovers_hint_without_replaying_consent(self):
        subprocess.run(["git", "-c", "init.templateDir=", "init", "--quiet", str(self.root)], check=True, env=self.env)
        hint = self.root / ".keito/config.yml"
        hint.parent.mkdir()
        old = b'{"project_id":"old","agent_tracking":{"enabled":false}}'
        for sig in (signal.SIGTERM, signal.SIGKILL):
            with self.subTest(signal=int(sig)):
                hint.write_bytes(old)
                marker = self.root / ("setup-started-" + str(int(sig)))
                env = {**self.env, "FAKE_PICKER": "1", "FAKE_SETUP_MARKER": str(marker), "FAKE_SETUP_SLEEP": "3"}
                child = subprocess.Popen(["bash", str(SKILL / "scripts/setup-wizard.sh"), "--agent", "codex"],
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, cwd=self.root,
                    start_new_session=True)
                child.stdin.write("1\n1\n1\ny\n")
                child.stdin.flush()
                deadline = time.monotonic() + 4
                while not marker.exists() and time.monotonic() < deadline and child.poll() is None:
                    time.sleep(0.01)
                self.assertTrue(marker.exists(), "Fixture must reach actual core setup before signal")
                os.killpg(child.pid, sig)
                child.communicate(timeout=4)
                if sig == signal.SIGKILL:
                    count_before = len([c for c in self.recorded() if "setup" in c["args"]])
                    recovery = self.run_script("scripts/setup-wizard.sh", "--agent", "codex", payload="1\n1\n1\ny\n",
                        env={**self.env, "FAKE_PICKER": "1"})
                    self.assertNotEqual(recovery.returncode, 0)
                    self.assertIn("status", recovery.stderr.lower())
                    self.assertEqual(len([c for c in self.recorded() if "setup" in c["args"]]), count_before)
                self.assertEqual(hint.read_bytes(), old)
                self.assertFalse((hint.parent / ".capture-mapping-recovery.json").exists())

    def test_external_mapping_edit_during_publish_or_restore_fsync_is_preserved(self):
        # Run the real entrypoint. Inject only an external writer immediately
        # after fsync of the candidate/restoration temporary file.
        wrapper = r'''
import os, pathlib, runpy, sys, tempfile
root = pathlib.Path(os.environ['FIXTURE_ROOT'])
hint = root / '.keito/config.yml'
real_mkstemp, real_fsync = tempfile.mkstemp, os.fsync
temporary_files = {}
fired = False
external = b'{"workspace_id":"external","client_id":"c-external","project_id":"p-external","task_id":"t-external","agent_tracking":{"enabled":false}}\n'
def mkstemp(*args, **kwargs):
    fd, name = real_mkstemp(*args, **kwargs)
    temporary_files[fd] = pathlib.Path(name)
    return fd, name
def fsync(fd):
    global fired
    result = real_fsync(fd)
    temporary = temporary_files.get(fd)
    if not fired and temporary and temporary.exists():
        content = temporary.read_bytes()
        target = (b'"workspace_id": "co_test"' in content if os.environ['INJECT_PHASE'] == 'publish'
                  else content.startswith(b'{"workspace_id":"old"'))
        if target:
            fired = True
            hint.write_bytes(external)
            (root / 'external-edit-injected').write_bytes(external)
    return result
tempfile.mkstemp, os.fsync = mkstemp, fsync
script = sys.argv[1]
sys.argv = [script, '--root', str(root), '--workspace', 'co_test', '--client', 'c1',
            '--project', 'p1', '--task', 't1', '--agent', 'codex', '--confirm']
runpy.run_path(script, run_name='__main__')
'''
        for phase in ('publish', 'rollback'):
            with self.subTest(phase=phase):
                root = self.root / ('mapping-' + phase)
                directory = root / '.keito'
                directory.mkdir(parents=True)
                hint = directory / 'config.yml'
                old = b'{"workspace_id":"old","client_id":"c-old","project_id":"p-old","task_id":"t-old","agent_tracking":{"enabled":false}}\n'
                hint.write_bytes(old)
                before = len([c for c in self.recorded() if 'setup' in c['args']])
                result = subprocess.run([sys.executable, '-c', wrapper, str(SKILL / 'scripts/approve-mapping.py')],
                    env={**self.env, 'FIXTURE_ROOT': str(root), 'INJECT_PHASE': phase, 'FAKE_PICKER': '1',
                         'FAKE_SETUP_EXIT': '0' if phase == 'publish' else '9'},
                    capture_output=True, text=True, timeout=10)
                external = (root / 'external-edit-injected').read_bytes()
                self.assertNotEqual(result.returncode, 0, result.stderr)
                self.assertEqual(hint.read_bytes(), external)
                self.assertIn('preserved', result.stderr)
                self.assertIn('status', result.stderr)
                journal = directory / '.capture-mapping-recovery.json'
                self.assertTrue(journal.is_file())
                self.assertEqual(base64.b64decode(json.loads(journal.read_text())['original']), old)
                calls = [c for c in self.recorded() if 'setup' in c['args']]
                self.assertEqual(len(calls) - before, 0 if phase == 'publish' else 1)
                # A retry must retain the external hint/journal and never replay
                # consent while operator inspection remains necessary.
                retry = subprocess.run([sys.executable, str(SKILL / 'scripts/approve-mapping.py'),
                    '--root', str(root), '--workspace', 'co_test', '--client', 'c1',
                    '--project', 'p1', '--task', 't1', '--agent', 'codex', '--confirm'],
                    env=self.env, capture_output=True, text=True, timeout=10)
                self.assertNotEqual(retry.returncode, 0)
                self.assertEqual(hint.read_bytes(), external)
                self.assertTrue(journal.is_file())
                self.assertEqual(len([c for c in self.recorded() if 'setup' in c['args']]), len(calls))

    def test_home_alias_is_allowed_but_dangling_child_symlink_is_refused(self):
        alias = self.root / "approved-home-alias"
        alias.symlink_to(self.home, target_is_directory=True)
        env = {**self.env, "HOME": str(alias)}
        result = self.run_script("installers/install-codex.sh", "--harness-version", "0.167.0", env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        other = self.home / ".claude"
        other.symlink_to(self.root / "missing-outside", target_is_directory=True)
        result = self.install("claude-code")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "missing-outside").exists())

    def test_symlinked_parent_of_existing_install_refuses_update_and_removal(self):
        self.assertEqual(self.install().returncode, 0)
        host = self.home / ".codex"
        external = self.root / "existing-install-outside"
        host.rename(external)
        host.symlink_to(external, target_is_directory=True)
        before = (external / "hooks.json").read_bytes()
        for result in (self.install(), self.uninstall()):
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("Traceback", result.stderr)
        self.assertEqual((external / "hooks.json").read_bytes(), before)
        self.assertTrue((external / "skills/keito-time-track/SKILL.md").is_file())

    def test_background_uploader_is_only_forwarded_when_explicitly_selected_and_previewed(self):
        subprocess.run(["git", "-c", "init.templateDir=", "init", "--quiet", str(self.root)], check=True, env=self.env)
        result = self.run_script("scripts/setup-wizard.sh", "--agent", "codex", "--install-worker", payload="1\n1\n1\ny\n",
                                 env={**self.env, "FAKE_PICKER": "1"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("background uploader", result.stdout)
        setup = [c for c in self.recorded() if "setup" in c["args"]]
        self.assertEqual(len(setup), 1)
        self.assertIn("--install-worker", setup[0]["args"])

    def test_executing_shell_substitution_is_preserved_as_ambiguous_hook(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        current = self.read_config()
        command = current["hooks"]["Stop"][0]["hooks"][0]["command"]
        literal_argv = shlex.split(command)[1:]
        marker = self.root / "audit-helper-ran"
        helper = self.root / "audit-helper"
        helper.write_text("#!/bin/sh\nprintf ran > " + shlex.quote(str(marker)) + "\nprintf '%s\\n' " + shlex.quote(str(self.bin)) + "\n")
        helper.chmod(0o700)
        extra = {"type": "command", "command": "KEITO_CLI_BIN=$(audit-helper) " + shlex.join(literal_argv)}
        current["hooks"]["Stop"].append({"hooks": [extra]})
        config.write_text(json.dumps(current))
        env = {**self.env, "PATH": str(self.root) + os.pathsep + self.env["PATH"]}
        executed = subprocess.run(["bash", "-c", extra["command"]], input="{}", capture_output=True, text=True, env=env, timeout=2)
        self.assertEqual((executed.returncode, executed.stdout, executed.stderr), (0, "", ""))
        self.assertEqual(marker.read_text(), "ran")
        self.assertEqual(self.install().returncode, 0)
        self.assertIn(extra, [h for g in self.read_config()["hooks"]["Stop"] for h in g["hooks"]])
        self.assertEqual(self.uninstall().returncode, 0)
        self.assertIn(extra, [h for g in self.read_config()["hooks"]["Stop"] for h in g["hooks"]])

    def test_external_settings_edit_after_journal_fsync_is_preserved_and_reported(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        journal = config.parent / ".keito-capture-publication.json"
        external = b'{"external":"latest-setting-after-journal"}'
        spec = importlib.util.spec_from_file_location("installer_late_writer", SKILL / "installers/manage-hooks.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        fsync = module.os.fsync
        changed = False
        def edit_after_fsync(fd):
            nonlocal changed
            fsync(fd)
            if journal.exists() and not changed:
                changed = True
                config.write_bytes(external)
        args = SimpleNamespace(confirm_migration=False, harness_version="0.167.1")
        with mock.patch.dict(os.environ, self.env, clear=True), mock.patch.object(module.os, "fsync", side_effect=edit_after_fsync):
            with self.assertRaises(module.SafeError):
                module.apply("codex", "install", args, str(self.bin))
        self.assertTrue(changed)
        self.assertEqual(config.read_bytes(), external)
        self.assertTrue((self.home / ".codex/skills/keito-time-track/SKILL.md").is_file())

    def test_generation_without_execute_permission_cannot_be_reused_as_installed(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        before = config.read_bytes()
        command = self.read_config()["hooks"]["Stop"][0]["hooks"][0]["command"]
        executable = Path(next(p for p in shlex.split(command) if p.endswith('/hooks/dispatch.sh')))
        executable.chmod(0o600)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Traceback", result.stderr)
        self.assertIn("review", result.stderr.lower())
        self.assertEqual(config.read_bytes(), before)
        self.assertEqual(executable.stat().st_mode & 0o777, 0o600)

    def test_deep_install_and_mapping_recovery_metadata_fail_without_traceback(self):
        self.assertEqual(self.install().returncode, 0)
        config = self.config_path("codex")
        before = config.read_bytes()
        nested = b'[' * 1200 + b'0' + b']' * 1200
        journal = config.parent / ".keito-capture-publication.json"
        journal.write_bytes(nested)
        result = self.uninstall()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Traceback", result.stderr)
        self.assertNotIn(str(self.home), result.stderr)
        self.assertEqual((config.read_bytes(), journal.read_bytes()), (before, nested))
        subprocess.run(["git", "-c", "init.templateDir=", "init", "--quiet", str(self.root)], check=True, env=self.env)
        directory = self.root / ".keito"
        directory.mkdir()
        hint = directory / "config.yml"
        old = b'{"project_id":"preserve"}'
        hint.write_bytes(old)
        mapping_journal = directory / ".capture-mapping-recovery.json"
        mapping_journal.write_bytes(nested)
        result = self.run_script("scripts/setup-wizard.sh", "--agent", "codex", payload="1\n1\n1\ny\n", env={**self.env, "FAKE_PICKER": "1"})
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Traceback", result.stderr)
        self.assertEqual((hint.read_bytes(), mapping_journal.read_bytes()), (old, nested))

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
