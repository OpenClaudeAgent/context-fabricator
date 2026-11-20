#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"
source "$(dirname "$0")/../m08/profile.sh"
source "$(dirname "$0")/profile.sh"

require_mac
app=${1:-"$quality_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"}
assets="$quality_dir/m09-assets"
mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/m09.XXXXXX")
echo "M09 reports in $reports"
/usr/bin/codesign --verify --deep --strict "$app"
{
    "$assets/llama-server" --version
    "$assets/llama-server" --list-devices
    sw_vers
    sysctl -n machdep.cpu.brand_string
    sysctl -n hw.memsize
    printf 'Generation model revision %s\n' "$m09_revision"
    printf 'Embedding model revision %s\n' "$m08_revision"
    printf '%s  %s\n' "$m09_sha256" "$assets/$m09_model" | shasum -a 256 -c
    printf '%s  %s\n' "$m09_alternate_sha256" "$assets/$m09_alternate" | shasum -a 256 -c
    printf '%s  %s\n' "$m08_sha256" "$assets/$m08_model" | shasum -a 256 -c
} 2>&1 | tee "$reports/profile.log"
grep -Fq "build $m08_build, commit $m08_runtime_commit" "$reports/profile.log"
grep -Eq '^ *MTL0:' "$reports/profile.log"

cd "$quality_root/backend"
CTXFAB_TEST_APP_BUNDLE="$app" CTXFAB_TEST_INFERENCE_ASSETS="$assets"     cargo test --locked -p ctxfab-server --test answers     packaged_app_supervises_workers_and_qualifies_answers -- --ignored --exact --nocapture     2>&1 | tee "$reports/packaged.log"
sed -n 's/^M09_RESULT //p' "$reports/packaged.log" > "$reports/results.json"
jq -e '.workers == 2 and (.cases | length >= 6) and .controls.returnedToInitial and .controls.embeddingWorkerPreserved and .connectedSources == 0 and .outboundAttempts == 0'     "$reports/results.json" >/dev/null
