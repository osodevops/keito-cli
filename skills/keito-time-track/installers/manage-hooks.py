#!/usr/bin/env python3
"""Install only selected Keito adapters; never grant host trust or mapping consent."""
import argparse
import copy
import fcntl
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
import uuid

SOURCE = Path(__file__).resolve().parents[1]
CONTRACT = 1
OWNED_PLUGIN = 'keito-time-track@keito'
SCRIPTS = {'dispatch.sh', 'session-start.sh', 'session-end.sh', 'session-finalize.sh',
           'prompt.sh', 'stop.sh', 'interrupt.sh'}
EVENTS = {'SessionStart': 'session-start', 'UserPromptSubmit': 'prompt',
          'Stop': 'stop', 'SessionEnd': 'session-end'}


class SafeError(Exception):
    pass


def read_config(file):
    if file.is_symlink():
        raise SafeError('Refusing a symlinked hook configuration. Restore a regular file before retrying.')
    if not file.exists():
        return {}, None
    original = file.read_bytes()
    try:
        data = json.loads(original)
    except (ValueError, UnicodeDecodeError):
        raise SafeError('Malformed hook JSON. Repair the existing file before retrying; it was not replaced.')
    if not isinstance(data, dict) or not isinstance(data.get('hooks', {}), dict):
        raise SafeError('Hook settings must contain JSON objects; original settings were not replaced.')
    for groups in data.get('hooks', {}).values():
        if not isinstance(groups, list):
            raise SafeError('Hook event settings must be arrays; original settings were not replaced.')
        for group in groups:
            if not isinstance(group, dict) or not isinstance(group.get('hooks'), list):
                raise SafeError('Malformed hook group; original settings were not replaced.')
            if any(not isinstance(hook, dict) for hook in group['hooks']):
                raise SafeError('Malformed hook definition; original settings were not replaced.')
    if 'enabledPlugins' in data and not isinstance(data['enabledPlugins'], dict):
        raise SafeError('Malformed plugin settings; original settings were not replaced.')
    return data, original


def owned_hook(hook, dest):
    command = hook.get('command')
    if hook.get('type') != 'command' or not isinstance(command, str):
        return False
    try:
        argv = shlex.split(command)
    except ValueError:
        return False
    return any(token == str(dest / 'hooks' / name) for token in argv for name in SCRIPTS)


def stripped(data, dest):
    result = copy.deepcopy(data)
    found_legacy = False
    if 'hooks' not in result:
        return result, found_legacy
    for event, groups in list(result['hooks'].items()):
        kept = []
        for group in groups:
            hooks = []
            for hook in group['hooks']:
                if owned_hook(hook, dest):
                    if str(dest / 'hooks/dispatch.sh') not in shlex.split(hook['command']):
                        found_legacy = True
                else:
                    hooks.append(hook)
            if hooks or not group['hooks']:
                kept.append({**group, 'hooks': hooks})
        if kept:
            result['hooks'][event] = kept
        else:
            del result['hooks'][event]
    return result, found_legacy


def tree_hashes(root):
    result = {}
    for file in sorted(root.rglob('*')):
        if file.is_symlink():
            raise SafeError('Symlinked skill content requires manual review; no settings were changed.')
        if file.is_file() and file != root / 'capture-install.json' and '__pycache__' not in file.parts and file.suffix != '.pyc':
            result[str(file.relative_to(root))] = hashlib.sha256(file.read_bytes()).hexdigest()
    return result


def receipt_valid(dest):
    file = dest / 'capture-install.json'
    if not file.is_file() or file.is_symlink():
        return False
    try:
        state = json.loads(file.read_text())
        return state.get('adapter_contract') == CONTRACT and state.get('files') == tree_hashes(dest)
    except (ValueError, OSError):
        return False


def check_runtime():
    binary = shutil.which(os.environ.get('KEITO_CLI_BIN', 'keito'))
    if not binary:
        raise SafeError('Install the Keito CLI with capture support, then retry.')
    try:
        result = subprocess.run([binary, 'capture', '--help'], stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=2)
    except (OSError, subprocess.TimeoutExpired):
        raise SafeError('Keito capture capability could not be checked. No hook settings were changed.')
    if result.returncode:
        raise SafeError('This Keito CLI lacks the capture command. Update it before installing adapters.')
    return str(Path(binary).absolute())


def harness_version(agent, explicit):
    if explicit:
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:-]{0,63}', explicit) or '..' in explicit:
            raise SafeError('Harness version is incompatible with the capture API identifier contract. Do not strip build metadata; qualify a supported release before retrying.')
        return explicit
    try:
        output = subprocess.run(['claude' if agent == 'claude-code' else 'codex', '--version'],
                                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=1)
        match = re.search(r'\b\d+\.\d+\.\d+(?:[-+][A-Za-z0-9._-]+)?\b', output.stdout)
        if output.returncode == 0 and match:
            return harness_version(agent, match[0])
    except (OSError, subprocess.TimeoutExpired):
        pass
    return 'unknown'


def sync_directory(directory):
    fd = os.open(directory, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def apply(agent, action, args, binary):
    home = Path.home()
    host = home / ('.claude' if agent == 'claude-code' else '.codex')
    config = host / ('settings.json' if agent == 'claude-code' else 'hooks.json')
    dest = host / 'skills/keito-time-track'
    if not host.exists() and action == 'uninstall':
        return
    host.mkdir(mode=0o700, parents=True, exist_ok=True)
    # OS-held lock releases after interruption; never break another installer lock.
    fd = os.open(str(config) + '.keito.lock', os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SafeError('Another Keito installer is active. Wait for it to finish before retrying.')
        data, original = read_config(config)
        if dest.is_symlink() or (dest.exists() and not dest.is_dir()):
            raise SafeError('The skill destination is not a regular directory. Review it before retrying.')
        next_data, legacy = stripped(data, dest)
        active_plugin = agent == 'claude-code' and data.get('enabledPlugins', {}).get(OWNED_PLUGIN) is True
        if action == 'install' and (legacy or active_plugin) and not args.confirm_migration:
            raise SafeError('Legacy Keito hooks/plugin detected. Review migration, then retry with --confirm-migration; unrelated hooks are preserved.')
        known_dest = dest.exists() and receipt_valid(dest)
        if action == 'uninstall' and dest.exists() and not known_dest:
            raise SafeError('Installed skill files were modified or predate the capture manifest. Preserve/review them before uninstalling; settings are unchanged.')
        if action == 'install' and dest.exists() and not known_dest and not args.confirm_migration:
            raise SafeError('Existing skill content requires review. Retry with --confirm-migration to preserve a backup and install the new adapter.')
        if active_plugin and action == 'install':
            next_data['enabledPlugins'][OWNED_PLUGIN] = False
        stage = backup = temporary = None
        installed = False
        committed = False
        version = harness_version(agent, args.harness_version) if action == 'install' else None
        try:
            if action == 'install':
                dest.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                stage = Path(tempfile.mkdtemp(prefix='.keito-time-track-stage-', dir=dest.parent))
                tree_hashes(SOURCE)  # refuse symlinked source before copying
                shutil.copytree(SOURCE, stage, dirs_exist_ok=True, ignore=shutil.ignore_patterns('__pycache__', '*.pyc', 'capture-install.json'))
                for file in stage.rglob('*'):
                    file.chmod(0o700 if file.is_dir() or file.suffix in ('.sh', '.py') else 0o600)
                state = {'adapter_contract': CONTRACT, 'agent': agent, 'harness_version': version,
                         'trusted': 'unknown', 'delivery_verified': False, 'files': tree_hashes(stage)}
                (stage / 'capture-install.json').write_text(json.dumps(state, indent=2, sort_keys=True) + '\n')
                (stage / 'capture-install.json').chmod(0o600)
                hooks = next_data.setdefault('hooks', {})
                events = {**EVENTS, 'StopFailure' if agent == 'claude-code' else 'Interrupt': 'interrupt'}
                for event, lifecycle in events.items():
                    command = 'KEITO_CLI_BIN=' + shlex.quote(binary) + ' ' + shlex.join([
                        str(dest / 'hooks/dispatch.sh'), '--agent', agent, '--event', lifecycle,
                        '--harness-version', version])
                    hooks.setdefault(event, []).append({'hooks': [{'type': 'command', 'command': command, 'timeout': 1}]})
            content = (json.dumps(next_data, indent=2, ensure_ascii=False) + '\n').encode()
            # Observe external configuration changes before replacing; this is not
            # an atomic CAS against unrelated writers, which do not take our lock.
            if (config.read_bytes() if config.exists() else None) != original:
                raise SafeError('Hook settings changed during preparation. Retry after reviewing the current file.')
            handle, name = tempfile.mkstemp(prefix='.' + config.name + '.keito-', dir=host)
            temporary = Path(name)
            with os.fdopen(handle, 'wb') as stream:
                stream.write(content)
                stream.flush()
                os.fsync(stream.fileno())
            if dest.exists():
                backup = dest.parent / ('.keito-time-track-backup-' + uuid.uuid4().hex)
                os.replace(dest, backup)
            if stage:
                os.replace(stage, dest)
                installed = True
                stage = None
            if (config.read_bytes() if config.exists() else None) != original:
                raise SafeError('Hook settings changed during preparation. Retry after reviewing the current file.')
            os.replace(temporary, config)
            temporary = None
            committed = True
            sync_directory(host)
        finally:
            if not committed:
                if installed:
                    shutil.rmtree(dest)
                if backup and backup.exists():
                    os.replace(backup, dest)
            if stage and stage.exists():
                shutil.rmtree(stage)
            if temporary and temporary.exists():
                temporary.unlink()
        if backup:
            if known_dest:
                try:
                    shutil.rmtree(backup)
                except OSError:
                    print('Installation completed; an old hidden payload backup needs manual cleanup.')
            else:
                print('Prior skill payload preserved in a hidden .keito-time-track-backup directory beside the installed skill.')
        print('Keito ' + agent + ' adapter ' + ('installed' if action == 'install' else 'uninstalled') + '.')
        if action == 'install':
            print('Harness version: ' + version + '. Trust/enabled and live delivery remain unverified.')
            print('Review normal harness trust/settings, then approve a repository mapping with keito capture setup.')
            if agent == 'codex':
                print('Codex features.hooks and hook trust are unchanged. Review/enable them in Codex; never bypass trust.')
    finally:
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['install', 'uninstall'])
    parser.add_argument('--agent', action='append', choices=['claude-code', 'codex'], required=True)
    parser.add_argument('--harness-version')
    parser.add_argument('--confirm-migration', action='store_true')
    args = parser.parse_args()
    binary = check_runtime() if args.action == 'install' else None
    for agent in dict.fromkeys(args.agent):
        apply(agent, args.action, args, binary)


if __name__ == '__main__':
    try:
        main()
    except SafeError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError):
        # Do not echo arbitrary JSON, paths or subprocess output on errors.
        print('Keito installer could not complete. Inspect local file permissions and retained backups before retrying.', file=sys.stderr)
        sys.exit(1)
