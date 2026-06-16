#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: pause-resume.sh pause|resume" >&2
}

action="${1:-}"
case "$action" in
  pause|resume) ;;
  *) usage; exit 1 ;;
esac

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck disable=SC1091
# shellcheck source=../hooks/lib/log.sh
. "$script_dir/../hooks/lib/log.sh"
# shellcheck disable=SC1091
# shellcheck source=../hooks/lib/config.sh
. "$script_dir/../hooks/lib/config.sh"

cwd=$(pwd -P)
state_file=$(keito_latest_state_for_cwd "$cwd" 2>/dev/null || true)
if [ -z "$state_file" ] || [ ! -f "$state_file" ]; then
  echo "No active Keito-tracked session state found for $cwd."
  exit 1
fi

paused=false
[ "$action" = "pause" ] && paused=true

jq --argjson paused "$paused" '.paused = $paused' "$state_file" > "$state_file.tmp"
mv "$state_file.tmp" "$state_file"

keito_log INFO "$action session state_file=$state_file"
echo "Keito tracking ${action}d for this session."
