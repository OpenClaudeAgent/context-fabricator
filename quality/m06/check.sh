#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"

require_mac
app="$quality_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/m06.XXXXXX")
echo "M06 embedded diagnostics reports in $reports"
/usr/bin/codesign --verify --deep --strict "$app"

cd "$quality_root/backend"
cargo test --locked -p ctxfab-server --lib telemetry -- --nocapture 2>&1 | tee "$reports/storage.log"
CTXFAB_TEST_APP_BUNDLE="$app" cargo test --locked -p ctxfab-server --test process \
    packaged_backend_persists_correlated_signals -- --ignored --exact --nocapture \
    2>&1 | tee "$reports/packaged.log"
grep -Fq 'Live helper correlated import and query spans' "$reports/packaged.log"
