#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck disable=SC1091
# shellcheck source=../hooks/lib/log.sh
. "$script_dir/../hooks/lib/log.sh"
# shellcheck disable=SC1091
# shellcheck source=../hooks/lib/config.sh
. "$script_dir/../hooks/lib/config.sh"

cwd=$(pwd -P)
config_path=$(keito_find_config "$cwd" 2>/dev/null || true)

if [ -z "$config_path" ] || [ ! -f "$config_path" ]; then
  echo "Keito tracking is already untracked for $cwd."
  exit 0
fi

echo "Keito tracking config found:"
echo "  $config_path"
printf 'Disable tracking for this repo by moving this config aside? [y/N] '
read -r answer

case "$answer" in
  y|Y|yes|YES) ;;
  *)
    echo "Disable cancelled; no changes written."
    exit 0
    ;;
esac

disabled_path="$config_path.disabled"
if [ -e "$disabled_path" ]; then
  disabled_path="$config_path.disabled.$(date -u +%Y%m%dT%H%M%SZ)"
fi

mv "$config_path" "$disabled_path"
keito_log INFO "disabled repo tracking config=$config_path disabled_path=$disabled_path"
echo "Disabled Keito tracking for this repo."
echo "Moved config to: $disabled_path"
