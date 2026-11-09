#!/usr/bin/env bash
# Already inside the single proved OS policy; never applies another sandbox.
set -euo pipefail
checker=$1
root=$2
cargo_bin=$3
shift 3
"$checker" verify "$root"
export KEITO_TEST_SERVICE_BOUNDARY_RECEIPT="$root/service-boundary-receipt.json"
"$cargo_bin" test --locked --all-features --all-targets "$@"
if [ "$#" -eq 0 ]; then
  # Finite socket-only tests; isolated startup, no external package imports.
  python3 -I -S -B tests/support/test_source_qa_protocol.py
  # Developer-only owned pair QA inherits this same proved policy; never nest it.
  PYTHONDONTWRITEBYTECODE=1 python3 tests/support/test_source_qa_contract.py "$root"
  PYTHONDONTWRITEBYTECODE=1 python3 tests/support/run_guarded_usage_qa.py \
    "$root" "$cargo_bin" "$(command -v rustc)"
fi
