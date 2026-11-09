#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in pause|resume) action=$1 ;; *) echo 'Usage: pause-resume.sh pause|resume' >&2; exit 1 ;; esac
shift
exec "${KEITO_CLI_BIN:-keito}" capture "$action" --repo "$@"
