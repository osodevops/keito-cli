#!/usr/bin/env bash
set -euo pipefail

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to merge Codex hook settings." >&2
  exit 1
fi

source_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
dest="$HOME/.codex/skills/keito-time-track"
hooks="$HOME/.codex/hooks.json"
keito_bin=${KEITO_CLI_BIN:-}
[ -n "$keito_bin" ] || keito_bin=$(command -v keito 2>/dev/null || true)

shell_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

mkdir -p "$(dirname "$dest")" "$(dirname "$hooks")"
dest_dir=$(cd "$(dirname "$dest")" && pwd -P)
dest_physical="$dest_dir/$(basename "$dest")"
if [ "$source_dir" != "$dest_physical" ]; then
  rm -rf "$dest"
  cp -R "$source_dir" "$dest"
fi

[ -f "$hooks" ] || printf '{}\n' > "$hooks"
keito_env=""
if [ -n "$keito_bin" ]; then
  keito_env="KEITO_CLI_BIN=$(shell_quote "$keito_bin") "
fi
start_cmd="${keito_env}KEITO_AGENT_TYPE=codex $(shell_quote "$dest/hooks/session-start.sh")"
end_cmd="${keito_env}KEITO_AGENT_TYPE=codex $(shell_quote "$dest/hooks/session-end.sh")"

jq \
  --arg start "$start_cmd" \
  --arg end "$end_cmd" \
  --arg start_script "$dest/hooks/session-start.sh" \
  --arg end_script "$dest/hooks/session-end.sh" \
  '
  .hooks = (.hooks // {}) |
  .hooks.SessionStart = (
    (.hooks.SessionStart // [])
    | map(select(
        (.hooks // [] | map(.command // "") | any(. == $start or contains($start_script)))
        | not
      ))
    + [{
      matcher: "startup|resume",
      hooks: [{ type: "command", command: $start }]
    }]
  ) |
  .hooks.Stop = (
    (.hooks.Stop // [])
    | map(select(
        (.hooks // [] | map(.command // "") | any(. == $end or contains($end_script)))
        | not
      ))
    + [{
      hooks: [{ type: "command", command: $end, timeout: 30 }]
    }]
  )
  ' "$hooks" > "$hooks.tmp"
mv "$hooks.tmp" "$hooks"

chmod +x "$dest/hooks/session-start.sh" "$dest/hooks/session-end.sh" "$dest/scripts/"*.sh

echo "Installed Keito Time Track skill for Codex:"
echo "  $dest"
echo "Updated hooks:"
echo "  $hooks"
