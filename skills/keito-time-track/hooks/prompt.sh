#!/usr/bin/env bash
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P) || exit 0
exec bash "$script_dir/dispatch.sh" --event prompt "$@"
