#!/usr/bin/env bash
# Development/release verification only. The driver preflights every mutation
# command; this wrapper hashes exactly the three operator config anchors.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
profile=${KEITO_TEST_REAL_PROFILE_HOME:?Set the original profile for digest-only verification}
[ "$#" -eq 2 ] || { echo 'Usage: qualify-capture-artifact.sh <fresh extracted artifact directory> <private run-owned root>' >&2; exit 2; }
fingerprint() {
  local relative file
  for relative in .codex/hooks.json .codex/config.toml .claude/settings.json; do
    file="$profile/$relative"
    [ ! -L "$file" ] || return 1
    if [ -f "$file" ]; then shasum -a 256 < "$file"; elif [ ! -e "$file" ]; then printf '%s\n' MISSING; else return 1; fi
  done
}
before=$(fingerprint)
readonly profile before
verify() {
  local result=$? after
  after=$(fingerprint) || result=98
  if [ "$after" != "$before" ]; then
    echo 'Operator profile changed during artifact verification; preserve evidence and stop.' >&2
    result=98
  fi
  exit "$result"
}
trap verify EXIT
cd "$repo"
CARGO_TARGET_DIR="$repo/target" cargo build --locked --features dev-tools --bin qualify-capture
driver="$repo/target/debug/qualify-capture"
if [ -x /usr/bin/sandbox-exec ]; then
  KEITO_QUALIFICATION_NETWORK_DENY=macos-sandbox-exec /usr/bin/sandbox-exec -p '(version 1) (allow default) (deny network*)' "$driver" "$1" "$2"
else
  KEITO_QUALIFICATION_NETWORK_DENY=unavailable "$driver" "$1" "$2"
fi
