#!/usr/bin/env bash
# Hooks only hand transient stdin to the local deterministic capture core.
# Never log, inspect transcripts, infer identity, or perform network writes here.
set -u
agent=${KEITO_AGENT_TYPE:-}
version=${KEITO_HARNESS_VERSION:-unknown}
event=""
while [ "$#" -gt 0 ]; do
  [ "$#" -ge 2 ] || exit 0
  case "$1" in
    --agent) agent=$2 ;;
    --event) event=$2 ;;
    --harness-version) version=$2 ;;
    *) exit 0 ;;
  esac
  shift 2
done
case "$agent" in claude-code|codex) ;; *) exit 0 ;; esac
case "$event" in session-start|prompt|stop|interrupt|session-end) ;; *) exit 0 ;; esac
[[ "$version" =~ ^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$ ]] || exit 0
[[ "$version" != *..* ]] || exit 0
keito_cmd=${KEITO_CLI_BIN:-keito}
guard_cmd=${KEITO_CAPTURE_GUARD_BIN:-}
case "$keito_cmd" in /*) ;; *) exit 0 ;; esac
case "$guard_cmd" in /*) ;; *) exit 0 ;; esac
# Core refusal belongs to the native guard, after durable publication.
[ -f "$guard_cmd" ] && [ -x "$guard_cmd" ] && [ ! -L "$guard_cmd" ] || exit 0
# Preserve the directly owned PID across shell -> native dispatch. There are
# no background timer processes. Exec refusal stays silent and fail-open; attached
# ownership sees EOF without terminal and cannot accept capture proof.
shopt -s execfail
exec "$guard_cmd" --internal-dispatch-v2 --core "$keito_cmd" --agent "$agent" --event "$event" --harness-version "$version" <&0 >/dev/null 2>&1
exit 0
