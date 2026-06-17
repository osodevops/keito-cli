#!/usr/bin/env bash
set -u

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=hooks/lib/log.sh
. "$script_dir/lib/log.sh"
# shellcheck source=hooks/lib/config.sh
. "$script_dir/lib/config.sh"
# shellcheck source=hooks/lib/duration.sh
. "$script_dir/lib/duration.sh"

payload=$(cat || true)

if ! keito_require_jq; then
  keito_log WARN "jq is not installed; skipping session end"
  exit 0
fi

cwd=$(keito_json_get_string "$payload" '.cwd // .workspace.current_dir // .workspace_root // .repository_root')
[ -n "$cwd" ] || cwd="$PWD"
cwd=$(cd "$cwd" 2>/dev/null && pwd -P) || exit 0

session_id=$(keito_session_id_from_payload "$payload" "$cwd")
safe_session_id=$(keito_safe_filename "$session_id")
state_file="$(keito_state_dir)/$safe_session_id.json"
if [ ! -f "$state_file" ]; then
  state_file=$(keito_latest_state_for_cwd "$cwd" 2>/dev/null || true)
fi
[ -f "$state_file" ] || exit 0

stored_session_id=$(jq -r '.session_id // empty' "$state_file")
[ -n "$stored_session_id" ] && session_id="$stored_session_id"

paused=$(jq -r '.paused // false' "$state_file")
if [ "$paused" = "true" ]; then
  keito_log INFO "skipped paused session session_id=$session_id"
  rm -f "$state_file"
  exit 0
fi

started_at=$(jq -r '.started_at // empty' "$state_file")
started_epoch=$(jq -r '.started_epoch // empty' "$state_file")
config_path=$(jq -r '.config_path // empty' "$state_file")
[ -f "$config_path" ] || config_path=$(keito_find_config "$cwd" 2>/dev/null || true)

if [ ! -f "$config_path" ]; then
  keito_log WARN "missing config for session session_id=$session_id cwd=$cwd"
  exit 0
fi

enabled=$(keito_yaml_get "$config_path" "agent_tracking.enabled" "true")
if [ "$enabled" = "false" ]; then
  keito_log INFO "skipped disabled config session_id=$session_id"
  rm -f "$state_file"
  exit 0
fi

now_epoch=$(keito_now_epoch)
if [ -z "$started_epoch" ] || ! [[ "$started_epoch" =~ ^[0-9]+$ ]]; then
  started_epoch=$now_epoch
fi
duration_seconds=$(( now_epoch - started_epoch ))
[ "$duration_seconds" -lt 0 ] && duration_seconds=0

min_duration=$(keito_yaml_get "$config_path" "agent_tracking.min_duration_seconds" "60")
[[ "$min_duration" =~ ^[0-9]+$ ]] || min_duration=60
if [ "$duration_seconds" -lt "$min_duration" ]; then
  keito_log INFO "skipped short session session_id=$session_id duration=${duration_seconds}s min=${min_duration}s"
  rm -f "$state_file"
  exit 0
fi

max_duration=$(keito_yaml_get "$config_path" "agent_tracking.max_duration_seconds" "28800")
[[ "$max_duration" =~ ^[0-9]+$ ]] || max_duration=28800
duration_capped=false
original_duration_seconds=$duration_seconds
if [ "$duration_seconds" -gt "$max_duration" ]; then
  duration_seconds=$max_duration
  duration_capped=true
fi

workspace_id=$(keito_yaml_get "$config_path" "workspace_id" "")
project_id=$(keito_yaml_get "$config_path" "project_id" "")
task_id=$(keito_yaml_get "$config_path" "task_id" "")
source=$(keito_yaml_get "$config_path" "agent_tracking.source" "agent")
case "$source" in
  web|cli|api|agent) ;;
  *) source="agent" ;;
esac

if [ -z "$project_id" ] || [ -z "$task_id" ]; then
  keito_log WARN "missing project_id or task_id in $config_path; preserving state session_id=$session_id"
  exit 0
fi

redact_notes=$(keito_yaml_get "$config_path" "agent_tracking.redact_notes" "false")
transcript=$(keito_json_get_string "$payload" '.transcript_path // .transcriptPath // empty')
notes=$(keito_extract_notes "$payload" "$transcript" "$cwd" "$redact_notes")
ended_at=$(keito_now_iso)
agent_id=${KEITO_AGENT_ID:-}
[ -n "$agent_id" ] || agent_id=$(keito_json_get_string "$payload" '.agent_id // .agent.id // empty')
agent_type=${KEITO_AGENT_TYPE:-}
[ -n "$agent_type" ] || agent_type=$(jq -r '.agent_type // "unknown"' "$state_file")
git_rev=$(jq -r '.git_rev // empty' "$state_file")
git_branch=$(jq -r '.git_branch // empty' "$state_file")
draft=$(keito_yaml_get "$config_path" "agent_tracking.draft" "true")
base_metadata=$(keito_yaml_metadata_json "$config_path")

metadata=$(jq -c -n \
  --argjson base "$base_metadata" \
  --arg skill "keito-time-track" \
  --arg session_id "$session_id" \
  --arg agent_id "$agent_id" \
  --arg agent_type "$agent_type" \
  --arg git_rev "$git_rev" \
  --arg git_branch "$git_branch" \
  --arg cwd "$cwd" \
  --arg config_path "$config_path" \
  --argjson duration_seconds "$duration_seconds" \
  --argjson original_duration_seconds "$original_duration_seconds" \
  --argjson duration_capped "$duration_capped" \
  --argjson draft "$draft" \
  '$base + {
    skill: $skill,
    session_id: $session_id,
    agent_id: $agent_id,
    agent_type: $agent_type,
    git_rev: $git_rev,
    git_branch: $git_branch,
    cwd: $cwd,
    config_path: $config_path,
    duration_seconds: $duration_seconds,
    original_duration_seconds: $original_duration_seconds,
    duration_capped: $duration_capped,
    draft: $draft
  }')

keito_cmd=${KEITO_CLI_BIN:-keito}
cmd=("$keito_cmd" --json)
if [ -n "$workspace_id" ]; then
  cmd+=(--workspace "$workspace_id")
fi
cmd+=(time session-record)
cmd+=(--project "$project_id")
cmd+=(--task "$task_id")
cmd+=(--session-id "$session_id")
cmd+=(--duration-seconds "$duration_seconds")
if [ -n "$started_at" ]; then
  cmd+=(--started-at "$started_at")
fi
cmd+=(--ended-at "$ended_at")
cmd+=(--source "$source")
cmd+=(--metadata "$metadata")
cmd+=(--notes "$notes")

if "${cmd[@]}" >> "$(keito_log_file)" 2>&1; then
  keito_log INFO "logged session session_id=$session_id duration=${duration_seconds}s source=$source"
  rm -f "$state_file"
else
  keito_log ERROR "failed to log session session_id=$session_id; state preserved at $state_file"
fi

exit 0
