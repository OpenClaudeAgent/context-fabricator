#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

require_mac
app="$quality_build_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
mkdir -p "$quality_dir/reports"
quality_begin_report reports outbound
echo "Outbound admission and scheduler reports in $reports"
/usr/bin/codesign --verify --deep --strict "$app"

cd "$quality_root/backend"
cargo test --locked -p ctxfab-server --test outbound --test scheduler -- --nocapture \
    2>&1 | tee "$reports/scenarios.log"
CTXFAB_TEST_APP_BUNDLE="$app" cargo test --locked -p ctxfab-server --test process \
    packaged_scheduler_recovers_incrementally_and_accounts_for_crash -- --ignored --exact --nocapture \
    2>&1 | tee "$reports/packaged.log"
grep -Fq 'Owner EOF joined the scheduler' "$reports/packaged.log"
