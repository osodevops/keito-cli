#!/usr/bin/env bash

keito_skill_home() {
  printf '%s\n' "${KEITO_SKILL_HOME:-$HOME/.keito/skill}"
}

keito_state_dir() {
  printf '%s\n' "$(keito_skill_home)/sessions"
}

keito_log_file() {
  printf '%s\n' "$(keito_skill_home)/skill.log"
}

keito_rotate_log_if_needed() {
  local file="$1"
  [ -f "$file" ] || return 0

  local size
  size=$(wc -c < "$file" 2>/dev/null || printf '0')
  if [ "${size:-0}" -gt 10485760 ]; then
    mv "$file" "$file.1" 2>/dev/null || true
  fi
}

keito_log() {
  local level="$1"
  shift
  local home log_file
  home=$(keito_skill_home)
  log_file=$(keito_log_file)

  mkdir -p "$home" "$(keito_state_dir)" 2>/dev/null || true
  keito_rotate_log_if_needed "$log_file"
  printf '%s [%s] %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$level" "$*" >> "$log_file" 2>/dev/null || true
}
