#!/usr/bin/env bash

keito_require_jq() {
  command -v jq >/dev/null 2>&1
}

keito_json_get() {
  local payload="$1"
  local filter="$2"
  printf '%s' "$payload" | jq -r "$filter" 2>/dev/null
}

keito_json_get_string() {
  local value
  value=$(keito_json_get "$1" "$2 // empty")
  [ "$value" != "null" ] && printf '%s\n' "$value"
}

keito_safe_filename() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_'
}

keito_session_id_from_payload() {
  local payload="$1"
  local cwd="$2"
  local raw
  raw=$(keito_json_get_string "$payload" '.session_id // .sessionId // .conversation_id // .conversationId // .transcript_path')
  if [ -n "$raw" ]; then
    printf '%s\n' "$raw"
    return 0
  fi

  if command -v shasum >/dev/null 2>&1; then
    printf '%s\n' "$cwd" | shasum | awk '{print "session_" $1}'
  else
    printf 'session_%s\n' "$(date -u +%s)"
  fi
}

keito_find_config() {
  local dir="$1"
  [ -n "$dir" ] || dir="$PWD"
  dir=$(cd "$dir" 2>/dev/null && pwd -P) || return 1

  local git_root=""
  git_root=$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null || true)

  while [ -n "$dir" ] && [ "$dir" != "/" ]; do
    if [ -f "$dir/.keito/config.yml" ]; then
      printf '%s\n' "$dir/.keito/config.yml"
      return 0
    fi
    if [ -n "$git_root" ] && [ "$dir" = "$git_root" ]; then
      break
    fi
    dir=$(dirname "$dir")
  done

  return 1
}

keito_yaml_unquote() {
  sed -e 's/^[[:space:]]*//' \
      -e 's/[[:space:]]*$//' \
      -e 's/^"//' \
      -e 's/"$//' \
      -e "s/^'//" \
      -e "s/'$//"
}

keito_yaml_get_fallback() {
  local file="$1"
  local path="$2"
  local default="$3"

  local section key value
  if [[ "$path" == *.* ]]; then
    section=${path%%.*}
    key=${path#*.}
    value=$(awk -v section="$section" -v key="$key" '
      $0 ~ "^[[:space:]]*" section ":" { in_section=1; next }
      in_section && $0 ~ "^[^[:space:]]" { in_section=0 }
      in_section && $0 ~ "^[[:space:]]+" key ":" {
        sub("^[[:space:]]+" key ":[[:space:]]*", "")
        sub("[[:space:]]+#.*$", "")
        print
        exit
      }
    ' "$file")
  else
    value=$(awk -v key="$path" '
      $0 ~ "^" key ":" {
        sub("^" key ":[[:space:]]*", "")
        sub("[[:space:]]+#.*$", "")
        print
        exit
      }
    ' "$file")
  fi

  if [ -n "$value" ]; then
    printf '%s\n' "$value" | keito_yaml_unquote
  else
    printf '%s\n' "$default"
  fi
}

keito_yaml_get() {
  local file="$1"
  local path="$2"
  local default="${3:-}"

  if command -v yq >/dev/null 2>&1; then
    local value
    value=$(yq -r ".$path // \"\"" "$file" 2>/dev/null || true)
    if [ -n "$value" ] && [ "$value" != "null" ]; then
      printf '%s\n' "$value"
      return 0
    fi
  fi

  keito_yaml_get_fallback "$file" "$path" "$default"
}

keito_yaml_metadata_json() {
  local file="$1"

  if command -v yq >/dev/null 2>&1; then
    local json
    json=$(yq -o=json '.metadata // {}' "$file" 2>/dev/null || true)
    if printf '%s' "$json" | jq -e 'type == "object"' >/dev/null 2>&1; then
      printf '%s\n' "$json"
      return 0
    fi
  fi

  local json key value
  json='{}'
  while IFS='=' read -r key value; do
    [ -n "$key" ] || continue
    json=$(printf '%s' "$json" | jq --arg key "$key" --arg value "$value" '. + {($key): $value}')
  done < <(awk '
    /^metadata:/ { in_section=1; next }
    in_section && /^[^[:space:]]/ { in_section=0 }
    in_section && /^[[:space:]]+[A-Za-z0-9_-]+:/ {
      line=$0
      sub(/^[[:space:]]+/, "", line)
      split(line, parts, ":")
      key=parts[1]
      sub(/^[^:]+:[[:space:]]*/, "", line)
      sub(/[[:space:]]+#.*$/, "", line)
      gsub(/^"|"$/, "", line)
      gsub(/^'\''|'\''$/, "", line)
      print key "=" line
    }
  ' "$file")

  printf '%s\n' "$json"
}

keito_latest_state_for_cwd() {
  local cwd="$1"
  local state_dir
  state_dir=$(keito_state_dir)
  [ -d "$state_dir" ] || return 1

  local candidate
  candidate=$(find "$state_dir" -type f -name '*.json' -print 2>/dev/null | while read -r file; do
    if [ "$(jq -r '.cwd // empty' "$file" 2>/dev/null)" = "$cwd" ]; then
      printf '%s\n' "$file"
    fi
  done | xargs ls -t 2>/dev/null | head -n 1)

  [ -n "$candidate" ] && printf '%s\n' "$candidate"
}
