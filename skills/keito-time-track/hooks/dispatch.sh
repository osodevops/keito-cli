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
command -v python3 >/dev/null 2>&1 || exit 0
python3 -I -S "${BASH_SOURCE[0]%/*}/guard.py" "$keito_cmd" "$agent" "$event" "$version" \
  <&0 >/dev/null 2>&1 &
worker=$!
# The guard publishes a locked, content-free flag before the heavy core starts.
# Only a private fixed-byte durable ACK permits exact flag cleanup. A timeout,
# missing executable or pre-handler death retains evidence for reconciliation.
# Budget TERM/KILL inside one second, including launch and cleanup margin.
(
  sleep 0.65
  kill -TERM "$worker" >/dev/null 2>&1 || exit 0
  sleep 0.025
  kill -KILL "$worker" >/dev/null 2>&1 || true
) >/dev/null 2>&1 &
watchdog=$!
wait "$worker" 2>/dev/null || true
kill "$watchdog" >/dev/null 2>&1 || true
wait "$watchdog" 2>/dev/null || true
exit 0
