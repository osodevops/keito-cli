#!/usr/bin/env bash
# Selection is a user workflow; consent and mapping validation belong to the CLI.
set -euo pipefail
keito_cmd=${KEITO_CLI_BIN:-keito}
agents=()
worker=()
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--install-worker' ]; then
    worker=(--install-worker)
    shift
    continue
  fi
  if [ "$1" != '--agent' ] || [ "$#" -lt 2 ]; then
    echo 'Usage: setup-wizard.sh --agent claude-code|codex [--agent claude-code|codex] [--install-worker]' >&2
    exit 1
  fi
  case "$2" in claude-code|codex) agents+=(--agent "$2") ;; *) echo 'Select a supported adapter explicitly.' >&2; exit 1 ;; esac
  shift 2
done
if [ "${#agents[@]}" -eq 0 ]; then
  echo 'Select the adapters explicitly: setup-wizard.sh --agent claude-code --agent codex' >&2
  exit 1
fi
command -v jq >/dev/null 2>&1 || { echo 'jq is required for the setup picker.' >&2; exit 1; }
"$keito_cmd" capture --help >/dev/null 2>&1 || { echo 'Update Keito CLI to a version with capture support.' >&2; exit 1; }
auth_json=$("$keito_cmd" --json auth status)
if ! jq -e '.authenticated == true and .api_key_valid == true' >/dev/null <<< "$auth_json"; then
  echo 'Verify authentication with keito auth login before setting up capture.' >&2
  exit 1
fi
repo_root=$(git rev-parse --show-toplevel) || { echo "Run setup from the intended Git repository." >&2; exit 1; }
repo_root=$(cd "$repo_root" && pwd -P)
workspace_id=$(jq -er '.account_id // .workspace_id // empty' <<< "$auth_json")
select_id() {
  local data=$1 label=$2 choice count
  count=$(jq -er 'if type == "array" then length else error("Expected a list") end' <<< "$data")
  if [ "$count" -eq 0 ]; then
    echo "No available $label. Add an existing authorized $label in Keito, then retry." >&2
    return 1
  fi
  echo "Select an existing $label:" >&2
  jq -r 'to_entries[] | "\(.key + 1)) \(.value.name) - \(.value.id)"' <<< "$data" >&2
  printf '%s number: ' "$label" >&2
  read -r choice
  [[ "$choice" =~ ^[0-9]{1,6}$ ]] || { echo 'Invalid selection.' >&2; return 1; }
  choice=$((10#$choice))
  if [ "$choice" -lt 1 ] || [ "$choice" -gt "$count" ]; then
    echo 'Invalid selection.' >&2
    return 1
  fi
  jq -er --argjson index "$((choice - 1))" '.[$index].id | select(type == "string" and length > 0)' <<< "$data"
}
clients=$("$keito_cmd" --json --workspace "$workspace_id" clients list)
client_id=$(select_id "$clients" client)
projects=$("$keito_cmd" --json --workspace "$workspace_id" projects list --client "$client_id")
project_id=$(select_id "$projects" project)
tasks=$("$keito_cmd" --json --workspace "$workspace_id" projects tasks "$project_id")
task_id=$(select_id "$tasks" task)
echo "Review mapping: workspace $workspace_id; client $client_id; project $project_id; task $task_id."
echo 'Capture sends allowlisted identities and observed interval metadata. Numeric usage is unavailable in this candidate.'
echo 'Prompts, responses, transcripts, code, full paths and branch names are not uploaded.'
echo 'Activity remains pending review and nonbillable. Existing repository config is only a mapping hint.'
if [ "${#worker[@]}" -gt 0 ]; then
  echo 'This setup also requests a local background uploader service for this user.'
else
  echo 'This setup approves local queuing only; automatic delivery requires explicit --install-worker setup.'
fi
if [ -e "$repo_root/.keito/config.yml" ] || [ -L "$repo_root/.keito/config.yml" ]; then
  echo 'An existing mapping hint will be replaced for setup; failure restores it. Inspect status after interruption.'
fi
printf 'Approve this repository mapping and metadata-only capture on this device? [y/N] '
read -r confirm
case "$confirm" in y|Y|yes|YES) ;; *) echo 'Setup cancelled; no mapping approved.'; exit 0 ;; esac
if [ "${#worker[@]}" -gt 0 ]; then
  agents+=(--install-worker)
fi
cd "$repo_root"
exec "$keito_cmd" --workspace "$workspace_id" capture setup \
  --client "$client_id" --project "$project_id" --task "$task_id" --write-hint \
  "${agents[@]}" --confirm
