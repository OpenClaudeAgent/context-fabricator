#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

require_mac
mkdir -p "$quality_dir/reports"
quality_begin_report reports product-static
echo "Product static checks in $reports"

(
    cd "$quality_root/desktop/macos"
    paths=(App UITests Client/Package.swift Client/Sources Client/Tests)
    xcrun swift-format lint --strict --configuration .swift-format --recursive "${paths[@]}"
    swiftlint lint --strict --no-cache --config .swiftlint.yml "${paths[@]}"
) 2>&1 | tee "$reports/swift.log"

for workspace in backend cli; do
    (
        cd "$quality_root/$workspace"
        cargo fmt --all --check
        cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
        cargo deny --locked --target aarch64-apple-darwin check
    ) 2>&1 | tee "$reports/$workspace.log"
done
