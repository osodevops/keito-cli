#!/usr/bin/env python3
"""Publish a mapping hint for core consent, restoring it on failed/interrupted setup."""
import argparse
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile


class SafeError(Exception):
    pass


class Interrupted(BaseException):
    def __init__(self, signum):
        self.signum = signum


def stop(signum, _frame):
    raise Interrupted(signum)


def checked(root, directory, hint, journal):
    if directory.is_symlink() or (directory.exists() and not directory.is_dir()) or hint.is_symlink() or journal.is_symlink():
        raise SafeError('Symlinked mapping/recovery paths require review; no hint was replaced.')
    if directory.parent != root:
        raise SafeError('Mapping must remain inside the approved repository.')


def sync_directory(directory):
    fd = os.open(directory, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic(file, content, mode=0o600):
    fd, name = tempfile.mkstemp(prefix='.capture-setup-', dir=file.parent)
    temporary = Path(name)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, 'wb') as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, file)
        sync_directory(file.parent)
    finally:
        if temporary.exists():
            temporary.unlink()


def restore(hint, journal):
    try:
        if journal.stat().st_size > 256 * 1024:
            raise ValueError()
        data = json.loads(journal.read_text())
        if not isinstance(data, dict) or set(data) != {'schema_version', 'original', 'mode', 'candidate_sha256'} or data['schema_version'] != 1:
            raise ValueError()
        original = None if data['original'] is None else base64.b64decode(data['original'], validate=True)
        if original is not None and len(original) > 64 * 1024:
            raise ValueError()
        if type(data['mode']) is not int or data['mode'] < 0 or data['mode'] > 0o777 or not re.fullmatch('[a-f0-9]{64}', data['candidate_sha256']):
            raise ValueError()
    except (ValueError, TypeError, OSError):
        raise SafeError('Mapping recovery metadata needs review. Inspect capture status before retrying.')
    current = hint.read_bytes() if hint.exists() else None
    if current == original:
        pass
    elif current is not None and hashlib.sha256(current).hexdigest() == data['candidate_sha256']:
        if original is None:
            hint.unlink()
        else:
            atomic(hint, original, data['mode'])
    else:
        raise SafeError('Mapping changed outside setup; it was preserved. Inspect capture status and recovery metadata before retrying.')
    journal.unlink()
    sync_directory(journal.parent)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True)
    for field in ('workspace', 'client', 'project', 'task'):
        parser.add_argument('--' + field, required=True)
    parser.add_argument('--agent', action='append', choices=['claude-code', 'codex'], required=True)
    parser.add_argument('--confirm', action='store_true', required=True)
    parser.add_argument('--install-worker', action='store_true')
    args = parser.parse_args()
    for value in (args.workspace, args.client, args.project, args.task):
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}', value) or '..' in value:
            raise SafeError('Mapping identifiers are invalid; no hint was replaced.')
    root = Path(args.root).resolve(strict=True)
    directory = root / '.keito'
    hint = directory / 'config.yml'
    journal = directory / '.capture-mapping-recovery.json'
    checked(root, directory, hint, journal)
    directory.mkdir(mode=0o700, exist_ok=True)
    lock = os.open(directory / '.capture-setup.lock', os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SafeError('Another mapping setup is active. Wait, then inspect capture status.')
        checked(root, directory, hint, journal)
        if journal.exists():
            restore(hint, journal)
            raise SafeError('Recovered the previous mapping after interrupted setup. Core consent may have committed; inspect keito capture status and explicitly reapprove before continuing.')
        original = hint.read_bytes() if hint.exists() else None
        if original is not None and len(original) > 64 * 1024:
            raise SafeError('The existing mapping hint is too large; review it before replacing.')
        mode = hint.stat().st_mode & 0o777 if hint.exists() else 0o600
        candidate = (json.dumps({'version': 1, 'workspace_id': args.workspace, 'client_id': args.client,
            'project_id': args.project, 'task_id': args.task,
            'agent_tracking': {'enabled': True, 'source': 'agent', 'redact_notes': True}}, indent=2) + '\n').encode()
        recovery = {'schema_version': 1, 'original': base64.b64encode(original).decode() if original is not None else None,
                    'mode': mode, 'candidate_sha256': hashlib.sha256(candidate).hexdigest()}
        atomic(journal, (json.dumps(recovery) + '\n').encode())
        child = None
        success = False
        try:
            checked(root, directory, hint, journal)
            if (hint.read_bytes() if hint.exists() else None) != original:
                raise SafeError('Mapping changed during preparation. Inspect capture status before retrying.')
            atomic(hint, candidate)
            command = [os.environ.get('KEITO_CLI_BIN', 'keito'), '--workspace', args.workspace, 'capture', 'setup']
            for agent in dict.fromkeys(args.agent):
                command += ['--agent', agent]
            command += ['--project', args.project, '--task', args.task, '--confirm']
            if args.install_worker:
                command.append('--install-worker')
            child = subprocess.Popen(command, cwd=root, stdin=subprocess.DEVNULL)
            result = child.wait()
            if result:
                print('Capture setup failed; the previous mapping will be restored. Core consent may have committed: inspect keito capture status and reapprove explicitly before continuing.', file=sys.stderr)
                return result if result > 0 else 128 - result
            checked(root, directory, hint, journal)
            if hint.read_bytes() != candidate:
                raise SafeError('Mapping changed while consent was being saved; inspect capture status before continuing.')
            journal.unlink()
            sync_directory(directory)
            success = True
            return 0
        finally:
            if child is not None and child.poll() is None:
                try:
                    child.terminate()
                except ProcessLookupError:
                    pass
                try:
                    child.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            if not success and journal.exists():
                checked(root, directory, hint, journal)
                restore(hint, journal)
    finally:
        os.close(lock)


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        sys.exit(main())
    except Interrupted as error:
        print('Setup interrupted; inspect keito capture status before reapproving. Any pending journal is recovered on the next setup.', file=sys.stderr)
        sys.exit(128 + error.signum)
    except SafeError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError):
        print('Setup could not complete safely. Review mapping/recovery files and keito capture status before retrying.', file=sys.stderr)
        sys.exit(1)
