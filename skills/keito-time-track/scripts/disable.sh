#!/usr/bin/env bash
set -euo pipefail
exec "${KEITO_CLI_BIN:-keito}" capture disable --repo "$@"
