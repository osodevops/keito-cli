#!/usr/bin/env python3
"""Install selected adapters with immutable hook generations and recoverable publication."""
import argparse
import copy
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import uuid

SOURCE = Path(__file__).resolve().parents[1]
CONTRACT = 1
_UNSET = object()
OWNED_PLUGIN = 'keito-time-track@keito'
SCRIPTS = {'dispatch.sh', 'session-start.sh', 'session-end.sh', 'session-finalize.sh', 'prompt.sh', 'stop.sh', 'interrupt.sh'}
EVENTS = {'SessionStart': 'session-start', 'UserPromptSubmit': 'prompt', 'Stop': 'stop', 'SessionEnd': 'session-end'}


class SafeError(Exception):
    pass


class Interrupted(BaseException):
    def __init__(self, signum):
        self.signum = signum


def stop(signum, _frame):
    raise Interrupted(signum)


def checked_paths(home, host, dest):
    # Resolve HOME itself once: normal macOS /tmp aliases are permitted. Refuse
    # any symlink below that approved anchor, including dangling links.
    for path in (host, host / 'skills', dest, host / '.keito-capture-generations'):
        if path.is_symlink() or (path.exists() and not path.is_dir()):
            raise SafeError('Symlinked or non-directory harness/skill parents require review; no installation or removal was performed.')
        if home not in path.parents:
            raise SafeError('The adapter destination is outside the approved home; review configuration before retrying.')


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
            if not isinstance(group, dict) or not isinstance(group.get('hooks'), list) or any(not isinstance(h, dict) for h in group['hooks']):
                raise SafeError('Malformed hook group; original settings were not replaced.')
    if 'enabledPlugins' in data and not isinstance(data['enabledPlugins'], dict):
        raise SafeError('Malformed plugin settings; original settings were not replaced.')
    return data, original


def literal_shell_command(command):
    """Conservative literal-word subset; shlex alone does not model expansion.

    Preserve ambiguous shell programs instead of deleting unrelated execution.
    Quoted literal metacharacters in actual file paths remain supported.
    """
    quote = None
    index = 0
    while index < len(command):
        char = command[index]
        if char in '\r\n':
            return False
        if quote == "'":
            if char == "'":
                quote = None
        elif char == "\\":
            index += 1
            if index >= len(command):
                return False
        elif char == "'" and quote is None:
            quote = "'"
        elif char == '"':
            quote = None if quote == '"' else '"'
        elif char in '$`' or (quote is None and char in ';&|<>(){}*?[]~#'):
            return False
        index += 1
    return quote is None


def owned_hook(hook, dest):
    """Recognize invoked executables/known integration arguments, never path data."""
    if hook.get('type') != 'command' or not isinstance(hook.get('command'), str) or not literal_shell_command(hook['command']):
        return False
    try:
        argv = shlex.split(hook['command'])
    except ValueError:
        return False
    if argv and argv[0] in ('env', '/usr/bin/env'):
        argv = argv[1:]
    environment = {}
    while argv and re.match(r'^[A-Za-z_][A-Za-z0-9_]*=', argv[0]):
        key, value = argv.pop(0).split('=', 1)
        if key not in ('KEITO_CLI_BIN', 'KEITO_AGENT_TYPE', 'KEITO_HARNESS_VERSION'):
            return False
        environment[key] = value
    if argv and argv[0] in ('bash', '/bin/bash', '/usr/bin/bash'):
        argv = argv[1:]
    if not argv:
        return False
    executable, args = Path(argv[0]).resolve(strict=False), argv[1:]
    host = dest.parent.parent
    agent = 'claude-code' if host.name == '.claude' else 'codex'
    legacy = executable.parent == dest / 'hooks' and executable.name in SCRIPTS
    generation = (executable.name == 'dispatch.sh' and executable.parent.name == 'hooks'
                  and executable.parent.parent.parent == host / '.keito-capture-generations'
                  and re.fullmatch(r'[a-f0-9]{64}', executable.parent.parent.name))
    if not legacy and not generation:
        return False
    if executable.name != 'dispatch.sh':
        return not args and environment.get('KEITO_AGENT_TYPE') == agent
    if len(args) != 6 or set(args[::2]) != {'--agent', '--event', '--harness-version'}:
        return False
    values = dict(zip(args[::2], args[1::2]))
    return (values['--agent'] == agent and values['--event'] in {*EVENTS.values(), 'interrupt'}
            and valid_version(values['--harness-version']))


def stripped(data, dest):
    result = copy.deepcopy(data)
    legacy = False
    for event, groups in list(result.get('hooks', {}).items()):
        kept = []
        for group in groups:
            hooks = []
            for hook in group['hooks']:
                if owned_hook(hook, dest):
                    legacy |= not any(token.endswith('/hooks/dispatch.sh') for token in shlex.split(hook['command']))
                else:
                    hooks.append(hook)
            if hooks or not group['hooks']:
                kept.append({**group, 'hooks': hooks})
        if kept:
            result['hooks'][event] = kept
        else:
            del result['hooks'][event]
    return result, legacy


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
        if file.stat().st_size > 512 * 1024:
            return False
        state = json.loads(file.read_text())
        if not isinstance(state, dict) or type(state.get('adapter_contract')) is not int or state['adapter_contract'] != CONTRACT:
            return False
        files = state.get('files')
        if not isinstance(files, dict) or not files:
            return False
        if any(not isinstance(name, str) or PurePosixPath(name).is_absolute() or '..' in PurePosixPath(name).parts
               or not isinstance(digest, str) or not re.fullmatch('[a-f0-9]{64}', digest) for name, digest in files.items()):
            return False
        # Content hashes do not establish that the referenced payload can run.
        # Verify regular files, readable/traversable directories and executable
        # shell entrypoints before reusing or reporting a generation installed.
        for directory in [dest, *[p for p in dest.rglob('*') if p.is_dir()]]:
            if directory.is_symlink() or (directory.stat().st_mode & 0o500) != 0o500:
                return False
        for name in files:
            file = dest / name
            mode = file.stat().st_mode
            if file.is_symlink() or not stat.S_ISREG(mode) or not mode & 0o400:
                return False
            if name.endswith('.sh') and (not mode & 0o100 or not os.access(file, os.X_OK)):
                return False
        return files == tree_hashes(dest)
    except (ValueError, OSError, RecursionError):
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
        raise SafeError('This Keito CLI lacks capture support. Update it before installing adapters.')
    return str(Path(binary).absolute())


def valid_version(value):
    return bool(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:-]{0,63}', value)) and '..' not in value


def harness_version(agent, explicit):
    if explicit:
        if not valid_version(explicit):
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


def atomic_bytes(file, content, expected=_UNSET):
    handle, name = tempfile.mkstemp(prefix='.' + file.name + '.keito-', dir=file.parent)
    temporary = Path(name)
    try:
        with os.fdopen(handle, 'wb') as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        if expected is not _UNSET and (file.is_symlink() or (file.read_bytes() if file.exists() else None) != expected):
            raise SafeError('Hook settings changed before publication. The external edit was preserved; review it before retrying.')
        # This last comparison detects journal/temp-fsync window edits. It is
        # not universal CAS against applications that ignore the install lock:
        # a writer can still race between this comparison and os.replace.
        os.replace(temporary, file)
        sync_directory(file.parent)
    finally:
        if temporary.exists():
            temporary.unlink()


def recover(host, dest):
    journal = host / '.keito-capture-publication.json'
    if not journal.exists() and not journal.is_symlink():
        return
    if journal.is_symlink():
        raise SafeError('Installation recovery metadata is symlinked; review it before retrying.')
    try:
        data = json.loads(journal.read_text())
        if not isinstance(data, dict) or set(data) != {'schema_version', 'stage', 'backup'} or data.get('schema_version') != 1:
            raise ValueError()
        if not re.fullmatch(r'\.keito-time-track-backup-[a-f0-9]{32}', data.get('backup', '')):
            raise ValueError()
        if data.get('stage') is not None and not re.fullmatch(r'\.keito-time-track-stage-[A-Za-z0-9_-]+', data['stage']):
            raise ValueError()
    except (ValueError, OSError, TypeError, RecursionError):
        raise SafeError('Installation recovery metadata needs review; no recovery paths were followed.')
    backup = dest.parent / data['backup']
    stage = dest.parent / data['stage'] if data['stage'] else None
    if backup.is_symlink() or (backup.exists() and not backup.is_dir()) or (stage and stage.is_symlink()):
        raise SafeError('Installation recovery paths are symlinked; review them before retrying.')
    if backup.exists() and not dest.exists():
        os.replace(backup, dest)
        sync_directory(dest.parent)
    if stage and stage.exists():
        if not receipt_valid(stage):
            raise SafeError('An interrupted staged payload needs review; it was preserved.')
        shutil.rmtree(stage)
    journal.unlink()
    sync_directory(host)


def prepare_payload(parent, agent, version):
    stage = Path(tempfile.mkdtemp(prefix='.keito-time-track-stage-', dir=parent))
    try:
        tree_hashes(SOURCE)
        shutil.copytree(SOURCE, stage, dirs_exist_ok=True,
                        ignore=shutil.ignore_patterns('__pycache__', '*.pyc', 'capture-install.json'))
        for file in stage.rglob('*'):
            file.chmod(0o700 if file.is_dir() or file.suffix in ('.sh', '.py') else 0o600)
        files = tree_hashes(stage)
        key = hashlib.sha256(json.dumps([agent, version, files], sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        receipt = {'adapter_contract': CONTRACT, 'agent': agent, 'harness_version': version,
                   'trusted': 'unknown', 'delivery_verified': False, 'generation': key, 'files': files}
        (stage / 'capture-install.json').write_text(json.dumps(receipt, indent=2, sort_keys=True) + '\n')
        (stage / 'capture-install.json').chmod(0o600)
        for file in stage.rglob('*'):
            if file.is_file():
                with file.open('rb') as stream:
                    os.fsync(stream.fileno())
        sync_directory(stage)
        return stage, key
    except BaseException:
        shutil.rmtree(stage)
        raise


def apply(agent, action, args, binary):
    home = Path.home().resolve(strict=True)
    host = home / ('.claude' if agent == 'claude-code' else '.codex')
    config = host / ('settings.json' if agent == 'claude-code' else 'hooks.json')
    dest = host / 'skills/keito-time-track'
    checked_paths(home, host, dest)
    if not host.exists() and action == 'uninstall':
        return
    host.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(str(config) + '.keito.lock', os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SafeError('Another Keito installer is active. Wait for it before retrying.')
        checked_paths(home, host, dest)
        recover(host, dest)
        data, original = read_config(config)
        next_data, legacy = stripped(data, dest)
        active_plugin = agent == 'claude-code' and data.get('enabledPlugins', {}).get(OWNED_PLUGIN) is True
        if action == 'install' and (legacy or active_plugin) and not args.confirm_migration:
            raise SafeError('Legacy Keito hooks/plugin detected. Review migration, then retry with --confirm-migration; unrelated hooks are preserved.')
        known_dest = dest.exists() and receipt_valid(dest)
        if dest.exists() and not known_dest and (action == 'uninstall' or not args.confirm_migration):
            raise SafeError('Installed skill files were modified or predate the capture manifest. Preserve/review them before continuing; settings are unchanged.')
        if active_plugin and action == 'install':
            next_data['enabledPlugins'][OWNED_PLUGIN] = False
        stage = None
        backup = dest.parent / ('.keito-time-track-backup-' + uuid.uuid4().hex)
        journal = host / '.keito-capture-publication.json'
        version = harness_version(agent, args.harness_version) if action == 'install' else None
        try:
            if action == 'install':
                generations = host / '.keito-capture-generations'
                generations.mkdir(mode=0o700, exist_ok=True)
                generation_stage, key = prepare_payload(generations, agent, version)
                generation = generations / key
                if generation.exists():
                    if generation.is_symlink() or not receipt_valid(generation) or tree_hashes(generation) != tree_hashes(generation_stage):
                        shutil.rmtree(generation_stage)
                        raise SafeError('An immutable adapter generation changed or cannot execute. Review trusted file permissions and release integrity before reinstalling; no hook configuration was changed.')
                    shutil.rmtree(generation_stage)
                else:
                    os.replace(generation_stage, generation)
                    sync_directory(generations)
                dest.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                stage = Path(tempfile.mkdtemp(prefix='.keito-time-track-stage-', dir=dest.parent))
                shutil.copytree(generation, stage, dirs_exist_ok=True)
                hooks = next_data.setdefault('hooks', {})
                events = {**EVENTS, 'StopFailure' if agent == 'claude-code' else 'Interrupt': 'interrupt'}
                for event, lifecycle in events.items():
                    command = 'KEITO_CLI_BIN=' + shlex.quote(binary) + ' ' + shlex.join([
                        str(generation / 'hooks/dispatch.sh'), '--agent', agent, '--event', lifecycle, '--harness-version', version])
                    hooks.setdefault(event, []).append({'hooks': [{'type': 'command', 'command': command, 'timeout': 1}]})
            checked_paths(home, host, dest)
            if (config.read_bytes() if config.exists() else None) != original:
                raise SafeError('Hook settings changed during preparation. Review them before retrying.')
            recovery = {'schema_version': 1, 'stage': stage.name if stage else None, 'backup': backup.name}
            atomic_bytes(journal, (json.dumps(recovery) + '\n').encode())
            # New registered executables are already durable and immutable.
            # Switch JSON before touching the discoverable skill directory.
            atomic_bytes(config, (json.dumps(next_data, indent=2, ensure_ascii=False) + '\n').encode(), expected=original)
            if dest.exists():
                os.replace(dest, backup)
            if stage:
                os.replace(stage, dest)
                stage = None
            if dest.parent.exists():
                sync_directory(dest.parent)
            journal.unlink()
            sync_directory(host)
        finally:
            # Normal exceptions/signals recover discovery. SIGKILL leaves the
            # durable journal for the next invocation; registered generations
            # remain available throughout every boundary.
            if journal.exists():
                recover(host, dest)
            elif stage and stage.exists():
                shutil.rmtree(stage)
        if backup.exists():
            if known_dest:
                shutil.rmtree(backup)
            else:
                print('Prior skill payload retained in a hidden .keito-time-track-backup directory for review.')
        print('Keito ' + agent + ' adapter ' + ('installed' if action == 'install' else 'uninstalled') + '.')
        if action == 'install':
            print('Harness version: ' + version + '. Trust/enabled and live delivery remain unverified.')
            print('Review normal host trust, then approve a repository mapping with keito capture setup.')
            if agent == 'codex':
                print('Codex features.hooks and hook trust are unchanged. Review/enable them in Codex; never bypass trust.')
        else:
            print('Recognized integration-owned registrations were removed. Immutable generations are retained for recovery; ambiguous/unrelated commands were preserved.')
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
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        main()
    except Interrupted as error:
        print('Keito installation was interrupted. Registered immutable payloads remain available; rerun the installer to recover discovery.', file=sys.stderr)
        sys.exit(128 + error.signum)
    except SafeError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError, RecursionError):
        print('Keito installer could not complete. Review local permissions and retained recovery files before retrying.', file=sys.stderr)
        sys.exit(1)
