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
  keito_log WARN "jq is not installed; skipping session start"
  exit 0
fi

cwd=$(keito_json_get_string "$payload" '.cwd // .workspace.current_dir // .workspace_root // .repository_root')
[ -n "$cwd" ] || cwd="$PWD"
cwd=$(cd "$cwd" 2>/dev/null && pwd -P) || exit 0

config_path=$(keito_find_config "$cwd" 2>/dev/null || true)
[ -n "$config_path" ] || exit 0

enabled=$(keito_yaml_get "$config_path" "agent_tracking.enabled" "true")
if [ "$enabled" = "false" ]; then
  exit 0
fi

session_id=$(keito_session_id_from_payload "$payload" "$cwd")
safe_session_id=$(keito_safe_filename "$session_id")
state_dir=$(keito_state_dir)
state_file="$state_dir/$safe_session_id.json"
mkdir -p "$state_dir" || exit 0

started_at=$(keito_now_iso)
started_epoch=$(keito_now_epoch)
agent_type=${KEITO_AGENT_TYPE:-}
if [ -z "$agent_type" ]; then
  agent_type=$(keito_json_get_string "$payload" '.agent_type // .agent.name // .agent // empty')
fi
[ -n "$agent_type" ] || agent_type="unknown"

jq -n \
  --arg session_id "$session_id" \
  --arg started_at "$started_at" \
  --argjson started_epoch "$started_epoch" \
  --arg cwd "$cwd" \
  --arg config_path "$config_path" \
  --arg git_rev "$(keito_git_rev "$cwd")" \
  --arg git_branch "$(keito_git_branch "$cwd")" \
  --arg agent_type "$agent_type" \
  '{
    session_id: $session_id,
    started_at: $started_at,
    started_epoch: $started_epoch,
    cwd: $cwd,
    config_path: $config_path,
    git_rev: $git_rev,
    git_branch: $git_branch,
    agent_type: $agent_type,
    paused: false
  }' > "$state_file.tmp" 2>/dev/null && mv "$state_file.tmp" "$state_file"

keito_log INFO "started session session_id=$session_id cwd=$cwd config=$config_path"
exit 0
