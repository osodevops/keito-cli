#!/usr/bin/env bash
set -euo pipefail

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to merge Claude Code hook settings." >&2
  exit 1
fi

source_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
dest="$HOME/.claude/skills/keito-time-track"
settings="$HOME/.claude/settings.json"
keito_bin=${KEITO_CLI_BIN:-}
[ -n "$keito_bin" ] || keito_bin=$(command -v keito 2>/dev/null || true)

shell_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

mkdir -p "$(dirname "$dest")" "$(dirname "$settings")"
dest_dir=$(cd "$(dirname "$dest")" && pwd -P)
dest_physical="$dest_dir/$(basename "$dest")"
if [ "$source_dir" != "$dest_physical" ]; then
  rm -rf "$dest"
  cp -R "$source_dir" "$dest"
fi

[ -f "$settings" ] || printf '{}\n' > "$settings"
keito_env=""
if [ -n "$keito_bin" ]; then
  keito_env="KEITO_CLI_BIN=$(shell_quote "$keito_bin") "
fi
start_cmd="${keito_env}KEITO_AGENT_TYPE=claude-code $(shell_quote "$dest/hooks/session-start.sh")"
end_cmd="${keito_env}KEITO_AGENT_TYPE=claude-code $(shell_quote "$dest/hooks/session-end.sh")"

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
      matcher: "*",
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
  ' "$settings" > "$settings.tmp"
mv "$settings.tmp" "$settings"

chmod +x "$dest/hooks/session-start.sh" "$dest/hooks/session-end.sh" "$dest/scripts/"*.sh

echo "Installed Keito Time Track skill for Claude Code:"
echo "  $dest"
echo "Updated hooks:"
echo "  $settings"
