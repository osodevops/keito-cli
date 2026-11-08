#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
exec python3 "$repo_root/skills/keito-time-track/tests/adapter_installer_test.py"
