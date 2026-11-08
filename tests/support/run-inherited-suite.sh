#!/usr/bin/env bash
# Already inside the single proved OS policy; never applies another sandbox.
set -euo pipefail
checker=$1
root=$2
cargo_bin=$3
shift 3
"$checker" verify "$root"
export KEITO_TEST_SERVICE_BOUNDARY_RECEIPT="$root/service-boundary-receipt.json"
exec "$cargo_bin" test --locked --all-features --all-targets "$@"
