#!/usr/bin/env bash
# Development only. Public services receive explicit TempDir profile roots;
# fault-process tests additionally validate HOME/CAPTURE before constructing a child.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
profile=${KEITO_TEST_REAL_PROFILE_HOME:?Set the original user-profile anchor for digest-only verification}
profile_fingerprint() {
  local file
  for file in "$profile/.codex/hooks.json" "$profile/.claude/settings.json"; do
    [ ! -L "$file" ] || return 1
    if [ -f "$file" ]; then
      shasum -a 256 < "$file" || return 1
    elif [ ! -e "$file" ]; then
      printf '%s\n' MISSING
    else
      return 1
    fi
  done
}
[ -d "$profile" ] || exit 1
baseline=$(profile_fingerprint)
readonly profile baseline
verify_baseline() {
  local result=$? after
  after=$(profile_fingerprint) || result=98
  if [ "$after" != "$baseline" ]; then
    echo 'Global profile changed during local verification; stop and preserve evidence.' >&2
    result=98
  fi
  exit "$result"
}
trap verify_baseline EXIT
cd "$repo"
cargo test --offline --lib skill_adapter::implementation::tests:: -- --nocapture
cargo test --offline --lib capture_mapping::implementation::tests:: -- --nocapture
