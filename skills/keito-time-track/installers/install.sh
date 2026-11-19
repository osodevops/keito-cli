#!/usr/bin/env bash
# Native CLI owns installation, preservation and recovery.
set -euo pipefail
exec "${KEITO_CLI_BIN:-keito}" skill install "$@"
