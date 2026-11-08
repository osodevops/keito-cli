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
command -v "$keito_cmd" >/dev/null 2>&1 || exit 0
"$keito_cmd" capture hook --agent "$agent" --event "$event" --harness-version "$version" \
  <&0 >/dev/null 2>&1 &
worker=$!
# The core commits SQLite locally before it acknowledges. Killing a stalled
# process is safe; the next event/status exposes missing or incomplete evidence.
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
