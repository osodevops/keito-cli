#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck disable=SC1091
# shellcheck source=../hooks/lib/log.sh
. "$script_dir/../hooks/lib/log.sh"
# shellcheck disable=SC1091
# shellcheck source=../hooks/lib/config.sh
. "$script_dir/../hooks/lib/config.sh"

cwd=$(pwd -P)
config_path=$(keito_find_config "$cwd" 2>/dev/null || true)
keito_cmd=${KEITO_CLI_BIN:-keito}

if [ -z "$config_path" ]; then
  echo "Keito tracking: untracked"
  echo "No .keito/config.yml found at or above $cwd"
  exit 0
fi

echo "Keito tracking: configured"
echo "Current directory: $cwd"
echo "Config: $config_path"
workspace_id=$(keito_yaml_get "$config_path" "workspace_id" "")
client_id=$(keito_yaml_get "$config_path" "client_id" "")
client_name=$(keito_yaml_get "$config_path" "client_name" "$client_id")
project_id=$(keito_yaml_get "$config_path" "project_id" "")
project_name=$(keito_yaml_get "$config_path" "project_name" "$project_id")
task_id=$(keito_yaml_get "$config_path" "task_id" "")
task_name=$(keito_yaml_get "$config_path" "task_name" "$task_id")
echo "Workspace: ${workspace_id:-default}"
echo "Client: $client_name ($client_id)"
echo "Project: $project_name ($project_id)"
echo "Task: $task_name ($task_id)"
echo "Enabled: $(keito_yaml_get "$config_path" "agent_tracking.enabled" "true")"
echo "Source: $(keito_yaml_get "$config_path" "agent_tracking.source" "agent")"
echo "Draft metadata: $(keito_yaml_get "$config_path" "agent_tracking.draft" "true")"
echo "Min duration: $(keito_yaml_get "$config_path" "agent_tracking.min_duration_seconds" "60")s"
echo "Max duration: $(keito_yaml_get "$config_path" "agent_tracking.max_duration_seconds" "28800")s"

state_file=$(keito_latest_state_for_cwd "$cwd" 2>/dev/null || true)
if [ -n "$state_file" ] && [ -f "$state_file" ]; then
  started_epoch=$(jq -r '.started_epoch // empty' "$state_file")
  session_id=$(jq -r '.session_id // empty' "$state_file")
  paused=$(jq -r '.paused // false' "$state_file")
  now=$(date -u +%s)
  elapsed=0
  if [[ "$started_epoch" =~ ^[0-9]+$ ]]; then
    elapsed=$((now - started_epoch))
  fi
  echo "Current session: $session_id (${elapsed}s elapsed, paused=$paused)"
else
  echo "Current session: none recorded"
fi

last_error=$(grep ' \[ERROR\] ' "$(keito_log_file)" 2>/dev/null | tail -n 1 || true)
if [ -n "$last_error" ]; then
  echo "Last hook error: $last_error"
else
  echo "Last hook error: none"
fi

if [ -n "${KEITO_CLI_BIN:-}" ] || command -v "$keito_cmd" >/dev/null 2>&1; then
  echo
  echo "Today's agent entries:"
  "$keito_cmd" --json time list --today --source agent --limit 20 2>/dev/null || echo "Unable to query Keito CLI."
fi
