#!/usr/bin/env bash
# Development-only entry point; native focused tests are run directly, never
# recursively from a cargo test process.
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
exec "$repo_root/tests/native-adapter-tests.sh"
