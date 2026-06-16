#!/usr/bin/env bash
set -euo pipefail

if command -v keito >/dev/null 2>&1; then
  echo "Keito CLI is already installed: $(command -v keito)"
  exit 0
fi

if command -v brew >/dev/null 2>&1; then
  brew install osodevops/tap/keito
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

url="https://github.com/osodevops/keito-cli/releases/latest/download/$asset"
archive="$tmp_dir/$asset"
curl -fsSL "$url" -o "$archive"
tar -xzf "$archive" -C "$tmp_dir"

keito_bin=$(find "$tmp_dir" -type f -name keito | head -n 1)
if [ -z "$keito_bin" ]; then
  echo "Downloaded Keito CLI archive did not contain a keito binary." >&2
  exit 1
fi

install_dir="${KEITO_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$install_dir"
cp "$keito_bin" "$install_dir/keito"
chmod +x "$install_dir/keito"

echo "Installed Keito CLI to $install_dir/keito"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    echo "Add $install_dir to PATH if keito is not found in new shells."
    ;;
esac
