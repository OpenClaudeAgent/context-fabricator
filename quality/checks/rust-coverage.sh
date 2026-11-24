#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

require_mac
mkdir -p "$quality_dir/reports"
quality_begin_report reports rust-coverage
work=$(quality_workspace rust-coverage)
quality_on_exit 'rm -rf -- "$work"'
echo "Rust workspace coverage reports in $reports"

for workspace in backend cli; do
    (
        cd "$quality_root/$workspace"
        CARGO_TARGET_DIR="$work/$workspace" cargo llvm-cov --locked --workspace --ignore-filename-regex '(^|/)tests/' \
            --fail-under-lines "$minimum_line_coverage"
    ) 2>&1 | tee "$reports/$workspace.log"
done
