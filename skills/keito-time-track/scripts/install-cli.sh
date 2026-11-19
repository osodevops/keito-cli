#!/usr/bin/env bash
set -euo pipefail

if command -v keito >/dev/null 2>&1; then
  if keito skill runtime --json >/dev/null 2>&1; then
    echo "Keito native runtime pair is already installed: $(command -v keito)"
    exit 0
  fi
  echo "The existing Keito installation is unqualified for capture; install the official paired release." >&2
  exit 1
fi

if command -v brew >/dev/null 2>&1; then
  brew install osodevops/tap/keito
  keito skill runtime --json >/dev/null
  exit 0
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required to install the Keito CLI without Homebrew." >&2
  exit 1
fi
if ! command -v tar >/dev/null 2>&1; then
  echo "tar is required to install the Keito CLI without Homebrew." >&2
  exit 1
fi

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
  Darwin:arm64) asset="keito-aarch64-apple-darwin.tar.gz" ;;
  Darwin:x86_64) asset="keito-x86_64-apple-darwin.tar.gz" ;;
  Linux:x86_64) asset="keito-x86_64-unknown-linux-gnu.tar.gz" ;;
  *)
    echo "Unsupported platform: $os $arch" >&2
    echo "Install manually from https://github.com/osodevops/keito-cli/releases/latest" >&2
    exit 1
    ;;
esac

tmp_dir=$(mktemp -d)
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

base="https://github.com/osodevops/keito-cli/releases/latest/download"
archive="$tmp_dir/$asset"
curl -fsSL "$base/$asset" -o "$archive"
curl -fsSL "$base/SHA256SUMS" -o "$tmp_dir/SHA256SUMS"
expected=$(awk -v name="$asset" '$2 == name { print $1 }' "$tmp_dir/SHA256SUMS")
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || { echo "Release checksum missing or ambiguous." >&2; exit 1; }
if command -v shasum >/dev/null 2>&1; then actual=$(shasum -a 256 "$archive" | awk '{print $1}'); else actual=$(sha256sum "$archive" | awk '{print $1}'); fi
[ "$actual" = "$expected" ] || { echo "Release archive checksum mismatch." >&2; exit 1; }
# Accept exactly the expected flat release layout, before extracting anything.
entries=$(tar -tzf "$archive")
[ "$(printf '%s\n' "$entries" | LC_ALL=C sort)" = "$(printf '%s\n' capture-runtime-manifest.json keito keito-capture-guard | LC_ALL=C sort)" ] || { echo "Unexpected release archive layout." >&2; exit 1; }
tar -xzf "$archive" -C "$tmp_dir"
for name in keito keito-capture-guard capture-runtime-manifest.json; do
  [ -f "$tmp_dir/$name" ] && [ ! -L "$tmp_dir/$name" ] || exit 1
done
"$tmp_dir/keito" skill runtime --json >/dev/null
install_dir="${KEITO_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$install_dir"
for name in keito keito-capture-guard capture-runtime-manifest.json; do
  [ ! -L "$install_dir/$name" ] || { echo "Refusing existing symlink destination." >&2; exit 1; }
  install -m 644 "$tmp_dir/$name" "$install_dir/$name"
done
chmod 755 "$install_dir/keito" "$install_dir/keito-capture-guard"
"$install_dir/keito" skill runtime --json >/dev/null

echo "Installed Keito CLI to $install_dir/keito"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    echo "Add $install_dir to PATH if keito is not found in new shells."
    ;;
esac
