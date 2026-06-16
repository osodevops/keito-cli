#!/usr/bin/env bash
set -euo pipefail

repo_root=$(pwd -P)
config_path="$repo_root/.keito/config.yml"
keito_cmd=${KEITO_CLI_BIN:-keito}

json_field() {
  local json=$1
  local filter=$2
  local label=$3
  local value

  if ! value=$(printf '%s' "$json" | jq -er "$filter // empty"); then
    echo "Keito CLI did not return $label." >&2
    exit 1
  fi

  printf '%s\n' "$value"
}

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required. Install jq and rerun /track-time-keito." >&2
  exit 1
fi

if [ -n "${KEITO_CLI_BIN:-}" ]; then
  if [ ! -x "$keito_cmd" ]; then
    echo "Keito CLI path is not executable: $keito_cmd" >&2
    exit 1
  fi
elif ! command -v "$keito_cmd" >/dev/null 2>&1; then
  echo "Keito CLI is required. Install it, then run: keito auth login" >&2
  exit 1
fi

auth_json=$("$keito_cmd" --json auth status 2>/dev/null || printf '{"authenticated":false}')
authenticated=$(printf '%s' "$auth_json" | jq -r '.authenticated // false')
api_key_valid=$(printf '%s' "$auth_json" | jq -r '.api_key_valid // true')
if [ "$authenticated" != "true" ] || [ "$api_key_valid" != "true" ]; then
  echo "Keito CLI is not authenticated. Run: keito auth login" >&2
  exit 1
fi

workspace_id=$(printf '%s' "$auth_json" | jq -r '.account_id // .workspace_id // empty')

echo "Keito time tracking setup for $(basename "$repo_root")"
echo

clients_json=$("$keito_cmd" --json clients list)
client_count=$(printf '%s' "$clients_json" | jq 'length')

echo "Select a client:"
if [ "$client_count" -gt 0 ]; then
  printf '%s' "$clients_json" | jq -r 'to_entries[] | "\(.key + 1)) \(.value.name) - \(.value.id)"'
fi
printf '%s) + Create new client\n' "$((client_count + 1))"
printf 'Client number: '
read -r client_choice
if ! [[ "$client_choice" =~ ^[0-9]+$ ]] || [ "$client_choice" -lt 1 ] || [ "$client_choice" -gt $((client_count + 1)) ]; then
  echo "Invalid client selection." >&2
  exit 1
fi

if [ "$client_choice" -eq $((client_count + 1)) ]; then
  printf 'Client name: '
  read -r client_name
  if [ -z "$client_name" ]; then
    echo "Client name is required." >&2
    exit 1
  fi
  client_json=$("$keito_cmd" --json clients create "$client_name")
  client_id=$(json_field "$client_json" '.id' 'created client id')
  client_name=$(json_field "$client_json" '.name' 'created client name')
else
  client_index=$((client_choice - 1))
  client_id=$(json_field "$clients_json" ".[$client_index].id" 'selected client id')
  client_name=$(json_field "$clients_json" ".[$client_index].name" 'selected client name')
fi

echo
projects_json=$("$keito_cmd" --json projects list --client "$client_id")
project_count=$(printf '%s' "$projects_json" | jq 'length')

echo "Select a project for $client_name:"
if [ "$project_count" -gt 0 ]; then
  printf '%s' "$projects_json" | jq -r '
    to_entries[]
    | "\(.key + 1)) \(.value.name) \((.value.code // "") | if . == "" then "" else "[" + . + "]" end) - \(.value.id)"
  '
fi
printf '%s) + Create new project\n' "$((project_count + 1))"
printf 'Project number: '
read -r project_choice
if ! [[ "$project_choice" =~ ^[0-9]+$ ]] || [ "$project_choice" -lt 1 ] || [ "$project_choice" -gt $((project_count + 1)) ]; then
  echo "Invalid project selection." >&2
  exit 1
fi

if [ "$project_choice" -eq $((project_count + 1)) ]; then
  printf 'Project name [%s]: ' "$(basename "$repo_root")"
  read -r project_name
  [ -n "$project_name" ] || project_name=$(basename "$repo_root")
  project_json=$("$keito_cmd" --json projects create "$project_name" --client "$client_id")
  project_id=$(json_field "$project_json" '.id' 'created project id')
  project_name=$(json_field "$project_json" '.name' 'created project name')
else
  project_index=$((project_choice - 1))
  project_id=$(json_field "$projects_json" ".[$project_index].id" 'selected project id')
  project_name=$(json_field "$projects_json" ".[$project_index].name" 'selected project name')
fi

tasks_json=$("$keito_cmd" --json projects tasks)
task_count=$(printf '%s' "$tasks_json" | jq 'length')
if [ "$task_count" -eq 0 ]; then
  echo "No Keito tasks were returned. Add a default task in Keito, then rerun this wizard." >&2
  exit 1
fi

echo
echo "Select a task:"
printf '%s' "$tasks_json" | jq -r 'to_entries[] | "\(.key + 1)) \(.value.name) - \(.value.id)"'
default_task=$(printf '%s' "$tasks_json" | jq -r '
  to_entries[]
  | select((.value.name | ascii_downcase) == "development")
  | .key + 1
' | head -n 1)
[ -n "$default_task" ] || default_task=1
printf 'Task number [%s]: ' "$default_task"
read -r task_choice
[ -n "$task_choice" ] || task_choice=$default_task
if ! [[ "$task_choice" =~ ^[0-9]+$ ]] || [ "$task_choice" -lt 1 ] || [ "$task_choice" -gt "$task_count" ]; then
  echo "Invalid task selection." >&2
  exit 1
fi
task_index=$((task_choice - 1))
task_id=$(json_field "$tasks_json" ".[$task_index].id" 'selected task id')
task_name=$(json_field "$tasks_json" ".[$task_index].name" 'selected task name')
client_name_yaml=$(jq -Rn --arg value "$client_name" '$value')
project_name_yaml=$(jq -Rn --arg value "$project_name" '$value')
task_name_yaml=$(jq -Rn --arg value "$task_name" '$value')

echo
printf 'Enable agent time tracking for this repo? [Y/n] '
read -r enabled_answer
enabled_answer=${enabled_answer:-Y}
case "$enabled_answer" in
  y|Y|yes|YES) enabled=true ;;
  *) enabled=false ;;
esac

echo
if [ -f "$config_path" ]; then
  echo "Existing Keito repo config will be replaced:"
  echo "  $config_path"
  echo
fi
echo "Review this repo-specific Keito config:"
echo "  Path: $config_path"
echo "  Workspace: ${workspace_id:-default}"
echo "  Client: $client_name ($client_id)"
echo "  Project: $project_name ($project_id)"
echo "  Task: $task_name ($task_id)"
echo "  Source: agent"
echo "  Draft metadata: true"
echo "  Min duration: 60s"
echo "  Max duration: 28800s"
printf 'Write this config for this repo only? [y/N] '
read -r confirm_answer
case "$confirm_answer" in
  y|Y|yes|YES) ;;
  *)
    echo "Setup cancelled; no changes written."
    exit 0
    ;;
esac

mkdir -p "$repo_root/.keito"
config_tmp=$(mktemp "$repo_root/.keito/config.yml.tmp.XXXXXX")
cleanup_config_tmp() {
  rm -f "$config_tmp"
}
trap cleanup_config_tmp EXIT

cat > "$config_tmp" <<EOF
version: 1
workspace_id: $workspace_id
client_id: $client_id
client_name: $client_name_yaml
project_id: $project_id
project_name: $project_name_yaml
task_id: $task_id
task_name: $task_name_yaml
agent_tracking:
  enabled: $enabled
  source: agent
  draft: true
  min_duration_seconds: 60
  max_duration_seconds: 28800
  redact_notes: false
metadata:
  integration: keito_skill
  skill: keito-time-track
EOF
mv "$config_tmp" "$config_path"
trap - EXIT

echo
echo "Wrote $config_path"
echo "Project: $project_name ($project_id)"
echo "Task: $task_name ($task_id)"
echo "Agent tracking: $enabled"
