#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/toolchain.sh"

require_mac
cd "$quality_root/desktop/macos"
paths=(App UITests Client/Package.swift Client/Sources Client/Tests)
xcrun swift-format lint --strict --configuration .swift-format --recursive "${paths[@]}"
swiftlint lint --strict --no-cache --config .swiftlint.yml "${paths[@]}"

mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/foundation.XXXXXX")
for workspace in backend cli; do
    (
        cd "$quality_root/$workspace"
        cargo fmt --all --check
        cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
        cargo deny --locked --target aarch64-apple-darwin check
        cargo llvm-cov --locked --workspace --ignore-filename-regex '(^|/)tests/' \
            --fail-under-lines "$minimum_line_coverage"
    ) 2>&1 | tee "$reports/$workspace.log"
done

cd "$quality_root"
make app-analysis
make app-unit
make integration
make app
make app-smoke
echo "Foundation checks passed. Rust reports in $reports"
