#!/usr/bin/env bash

keito_now_epoch() {
  date -u +%s
}

keito_now_iso() {
  date -u +'%Y-%m-%dT%H:%M:%SZ'
}

keito_git_rev() {
  git -C "$1" rev-parse --short HEAD 2>/dev/null || true
}

keito_git_branch() {
  git -C "$1" branch --show-current 2>/dev/null || true
}

keito_trim_notes() {
  awk 'BEGIN { max = 1500 } { text = text $0 "\n" } END { gsub(/^[ \t\r\n]+|[ \t\r\n]+$/, "", text); print substr(text, 1, max) }'
}

keito_notes_from_transcript() {
  local transcript="$1"
  [ -f "$transcript" ] || return 1

  jq -r '
    select((.role // .message.role // empty) == "assistant")
    | (.content // .message.content // .message.text // empty)
    | if type == "array" then map(.text // empty) | join(" ") else . end
  ' "$transcript" 2>/dev/null | tail -n 1
}

keito_extract_notes() {
  local payload="$1"
  local transcript="$2"
  local cwd="$3"
  local redact="$4"

  if [ "$redact" = "true" ]; then
    printf '[redacted]\n'
    return 0
  fi

  local notes
  notes=$(printf '%s' "$payload" | jq -r '.last_assistant_message // .summary // .message.content // empty' 2>/dev/null | keito_trim_notes)
  if [ -z "$notes" ] && [ -n "$transcript" ]; then
    notes=$(keito_notes_from_transcript "$transcript" | keito_trim_notes)
  fi
  if [ -z "$notes" ]; then
    notes="Agent coding session in $(basename "$cwd")"
  fi

  printf '%s\n' "$notes"
}
