#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

echo "==> Building release binary..."
cargo build --release

echo "==> Recording demo tapes..."
mkdir -p "$ROOT_DIR/recordings"

for tape in "$ROOT_DIR"/tapes/*.tape; do
    echo "  Recording $(basename "$tape")..."
    vhs "$tape"
done

echo "==> Optimising GIFs..."
for gif in "$ROOT_DIR"/recordings/*.gif; do
    echo "  Optimising $(basename "$gif")..."
    gifsicle --optimize=3 --lossy=80 -o "${gif%.gif}-optimised.gif" "$gif"
done

echo "==> Done. Recordings in $ROOT_DIR/recordings/"
