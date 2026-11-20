#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"

require_mac
app="$quality_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/m04.XXXXXX")
echo "M04 packaged corpus reports in $reports"
/usr/bin/codesign --verify --deep --strict "$app"

cd "$quality_root/backend"
CTXFAB_TEST_APP_BUNDLE="$app" cargo test --locked -p ctxfab-server --test process \
    packaged_helper_serves_fresh_and_migrated_corpus -- --ignored --exact --nocapture \
    2>&1 | tee "$reports/packaged.log"
grep -Fq 'Verified fresh packaged SQLite/FTS5' "$reports/packaged.log"
grep -Fq 'Verified migrated packaged SQLite/FTS5' "$reports/packaged.log"
