#!/usr/bin/env python3
"""Durable, content-free handoff guard. No JSON, accounting or network work."""
import fcntl
import os
from pathlib import Path
import pwd
import re
import secrets
import select
import signal
import stat
import subprocess
import sys
import time

PENDING = b'KEITO_CAPTURE_PENDING_V1\n'
ACK = b'KEITO_CAPTURE_DURABLE_V1\n'
PUBLICATION_LOCK = 'capture-invocations-publication.lock'
INVOCATIONS = 'capture-invocations'
UID = os.geteuid()


class GuardRefused(Exception):
    pass


class Interrupted(BaseException):
    pass


def interrupted(_signum, _frame):
    raise Interrupted()


def checked_directory(fd, private=False):
    info = os.fstat(fd)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid not in (0, UID):
        raise GuardRefused()
    if private and (info.st_uid != UID or stat.S_IMODE(info.st_mode) != 0o700):
        raise GuardRefused()


def child_directory(parent, name, private=False):
    try:
        os.mkdir(name, 0o700, dir_fd=parent)
        os.fsync(parent)
    except FileExistsError:
        pass
    fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
    try:
        checked_directory(fd, private)
        return fd
    except BaseException:
        os.close(fd)
        raise


def capture_directory():
    configured = os.environ.get('KEITO_CAPTURE_HOME')
    if configured is None:
        # HOME and cwd come from the host environment, not capture authority.
        # Explicit operational overrides remain available to tests/supervisors.
        home = Path(pwd.getpwuid(UID).pw_dir)
        configured = str(home / ('Library/Application Support' if sys.platform == 'darwin' else '.config') / 'keito/capture')
    if not configured.startswith('/') or any(part in ('.', '..') for part in configured.split('/')):
        raise GuardRefused()
    parts = Path(configured).parts[1:]
    if not parts:
        raise GuardRefused()
    parent = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        checked_directory(parent)
        for index, part in enumerate(parts):
            fd = child_directory(parent, part, private=index == len(parts) - 1)
            os.close(parent)
            parent = fd
        return parent
    except BaseException:
        os.close(parent)
        raise


def regular_file(fd):
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != UID or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600:
        raise GuardRefused()
    return info


def publication_lock(root):
    fd = os.open(PUBLICATION_LOCK, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=root)
    try:
        regular_file(fd)
        until = time.monotonic() + 0.025
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return fd
            except BlockingIOError:
                if time.monotonic() >= until:
                    os.close(fd)
                    return None
                time.sleep(0.001)
    except BaseException:
        os.close(fd)
        raise


def prepare(invocations, nonce):
    # The normal publisher holds the short publication lock, so a scanner never
    # mistakes create-before-flock for an abandoned normal invocation. A refused
    # lock acquisition deliberately uses this same append-only preparation as
    # emergency failure evidence and never launches a core.
    name = nonce + '.preparing'
    fd = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=invocations)
    try:
        regular_file(fd)
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        offset = 0
        while offset < len(PENDING):
            written = os.write(fd, PENDING[offset:])
            if written <= 0:
                raise GuardRefused()
            offset += written
        os.fsync(fd)
        os.fsync(invocations)
        return fd
    except BaseException:
        # Keep a classifiable zero/prefix preparation after interruption.
        os.close(fd)
        raise


def publish(root, invocations, nonce):
    lock = publication_lock(root)
    marker = None
    try:
        marker = prepare(invocations, nonce)
        if lock is None:
            # Durable refusal even when publication coordination is unavailable.
            os.close(marker)
            return None
        os.rename(nonce + '.preparing', nonce + '.pending', src_dir_fd=invocations, dst_dir_fd=invocations)
        os.fsync(invocations)
        return marker
    except BaseException:
        if marker is not None:
            os.close(marker)
        raise
    finally:
        if lock is not None:
            os.close(lock)


def remove_acknowledged(root, invocations, nonce, marker):
    lock = publication_lock(root)
    if lock is None:
        return
    try:
        name = nonce + '.pending'
        expected = regular_file(marker)
        current = os.stat(name, dir_fd=invocations, follow_symlinks=False)
        if (current.st_dev, current.st_ino) != (expected.st_dev, expected.st_ino) or not stat.S_ISREG(current.st_mode):
            raise GuardRefused()
        os.unlink(name, dir_fd=invocations)
        os.fsync(invocations)
    finally:
        os.close(lock)


def terminate(child):
    if child is None or child.poll() is not None:
        return
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        child.wait(timeout=0.01)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            child.wait(timeout=0.01)
        except subprocess.TimeoutExpired:
            pass


def main():
    started = time.monotonic()
    if len(sys.argv) != 5:
        return
    core, agent, event, version = sys.argv[1:]
    if agent not in ('claude-code', 'codex') or event not in ('session-start', 'prompt', 'stop', 'interrupt', 'session-end'):
        return
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:-]{0,63}', version) or '..' in version:
        return
    root = invocations = marker = read_ack = write_ack = None
    child = None
    try:
        root = capture_directory()
        invocations = child_directory(root, INVOCATIONS, private=True)
        with os.scandir(invocations) as entries:
            if sum(1 for _ in zip(entries, range(1024))) >= 1024:
                return  # Existing retained flags already block collection.
        nonce = secrets.token_hex(32)
        marker = publish(root, invocations, nonce)
        if marker is None or time.monotonic() - started >= 0.60:
            return
        read_ack, write_ack = os.pipe()
        os.set_blocking(read_ack, False)
        env = {**os.environ, 'KEITO_CAPTURE_INVOCATION_NONCE': nonce, 'KEITO_CAPTURE_ACK_FD': str(write_ack)}
        # stdin is inherited byte-for-byte. The guard never reads or stores it.
        # Only the private acknowledgement pipe is inherited beyond stdio; the
        # guard alone owns the marker lock, so killing it exposes abandonment.
        child = subprocess.Popen([core, 'capture', 'hook', '--agent', agent, '--event', event, '--harness-version', version],
            stdin=None, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            env=env, pass_fds=(write_ack,), start_new_session=True)
        os.close(write_ack)
        write_ack = None
        response = bytearray()
        eof = False
        while time.monotonic() - started < 0.60:
            if not eof and select.select([read_ack], [], [], 0.005)[0]:
                data = os.read(read_ack, len(ACK) + 1)
                if not data:
                    eof = True
                else:
                    response.extend(data)
                    if len(response) > len(ACK):
                        break
            if child.poll() is not None and eof:
                if child.returncode == 0 and bytes(response) == ACK:
                    remove_acknowledged(root, invocations, nonce, marker)
                return
            if eof:
                time.sleep(0.001)
    finally:
        terminate(child)
        for fd in (write_ack, read_ack, marker, invocations, root):
            if fd is not None:
                os.close(fd)


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        main()
    except (Interrupted, OSError, ValueError, GuardRefused):
        pass
