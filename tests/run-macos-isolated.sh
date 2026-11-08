#!/usr/bin/env bash
# Single inherited policy; never nest sandbox-exec inside an existing sandbox.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
profile=${KEITO_TEST_REAL_PROFILE_HOME:?Set the original profile for digest-only verification}
original_home=${HOME:?}
cargo_home=${CARGO_HOME:-$original_home/.cargo}
rustup_home=${RUSTUP_HOME:-$original_home/.rustup}
cargo_bin=$(command -v cargo)
cargo_jobs=${CARGO_BUILD_JOBS:-2}
test_threads=${RUST_TEST_THREADS:-1}
case "$cargo_jobs:$test_threads" in *[!0-9:]*|:*|*:) echo 'Concurrency bounds must be positive integers' >&2; exit 2;; esac
[ "$cargo_jobs" -ge 1 ] && [ "$cargo_jobs" -le 2 ] && [ "$test_threads" -eq 1 ] || { echo 'Use at most two Cargo jobs and exactly one test thread' >&2; exit 2; }
export CARGO_BUILD_JOBS="$cargo_jobs" RUST_TEST_THREADS="$test_threads"
profile_fingerprint() {
  local relative file
  for relative in .codex/hooks.json .codex/config.toml .claude/settings.json; do
    file="$profile/$relative"
    [ ! -L "$file" ] || return 1
    if [ -f "$file" ]; then shasum -a 256 < "$file"; elif [ ! -e "$file" ]; then printf '%s\n' MISSING; else return 1; fi
  done
}
before=$(profile_fingerprint)
readonly profile before
verify_profile() {
  local result=$? after
  after=$(profile_fingerprint) || result=98
  if [ "$after" != "$before" ]; then echo 'Operator profile changed; preserve evidence and stop.' >&2; result=98; fi
  exit "$result"
}
trap verify_profile EXIT
cd "$repo"
CARGO_TARGET_DIR="$repo/target" CARGO_NET_OFFLINE=true cargo build --locked --features dev-tools --bin check-service-boundary
root=$(mktemp -d /tmp/keito-native-tests.XXXXXX)
root=$(cd "$root" && pwd -P)
chmod 700 "$root"
"$repo/target/debug/check-service-boundary" prepare "$root"
printf 'Owned test boundary: %s\n' "$root"
# This is the only sandbox application. Both checker and Cargo inherit it.
env -i HOME="$root/home" CODEX_HOME="$root/home/.codex" CLAUDE_CONFIG_DIR="$root/home/.claude" \
  KEITO_CAPTURE_HOME="$root/home/capture" XDG_CONFIG_HOME="$root/home/config" APPDATA="$root/home/AppData" \
  GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null PATH="$PATH" \
  CARGO_HOME="$cargo_home" RUSTUP_HOME="$rustup_home" CARGO_TARGET_DIR="$repo/target" \
  CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS="$cargo_jobs" RUST_TEST_THREADS="$test_threads" RUSTFLAGS="${RUSTFLAGS:-}" RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}" \
  /usr/bin/sandbox-exec -f "$root/boundary.sb" /bin/bash "$repo/tests/support/run-inherited-suite.sh" \
  "$root/owned-checker" "$root" "$cargo_bin" "$@"
