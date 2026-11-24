#!/bin/bash
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=../toolchain/environment.sh
source "$(dirname "$0")/../toolchain/environment.sh"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=../retrieval/profile.sh
source "$quality_root/quality/retrieval/profile.sh"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=../answers/profile.sh
source "$quality_root/quality/answers/profile.sh"
require_mac

assets=${CTXFAB_M11_WORKLOAD_ASSETS:-"$quality_cache_dir/m09-assets"}
mkdir -p "$quality_dir/reports"
quality_begin_report report mixed-workload
echo "Mixed workload reports in $report"
jq -n --arg model "$(/usr/sbin/sysctl -n hw.model)" \
    --arg cpu "$(/usr/sbin/sysctl -n machdep.cpu.brand_string)" \
    --arg architecture "$(uname -m)" --arg os "$(sw_vers -productVersion)" \
    --argjson memoryBytes "$(/usr/sbin/sysctl -n hw.memsize)" \
    '{model: $model, cpu: $cpu, architecture: $architecture, os: $os, memoryBytes: $memoryBytes}' \
    > "$report/reference-host.json"
(cd "$assets" && printf '%s  %s\n' "$m08_sha256" "$m08_model" "$m09_sha256" "$m09_model" | shasum -a 256 -c) \
    | tee "$report/model-integrity.log"
bundle=${CTXFAB_M11_WORKLOAD_BUNDLE:-}
if [[ -z $bundle ]]; then
    source_app="$quality_build_dir/release-xcode/Build/Products/Release/Context Fabricator.app"
    work=$(quality_workspace workload)
    quality_on_exit 'rm -rf -- "$work"'
    bundle="$work/Context Fabricator Qualification.app"
    /usr/bin/codesign --verify --deep --strict "$source_app"
    CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true cargo build --manifest-path "$quality_root/backend/Cargo.toml" \
        --locked --release -p ctxfab-server > "$report/build.log" 2>&1
    ditto "$source_app" "$bundle"
    /usr/bin/install -m 755 "$quality_root/backend/target/release/ctxfab-server" "$bundle/Contents/MacOS/ctxfab-server"
    /usr/bin/codesign --force --sign - --identifier org.contextfabricator.backend "$bundle/Contents/MacOS/ctxfab-server"
    /usr/bin/codesign --force --sign - --preserve-metadata=identifier,entitlements,flags,runtime "$bundle"
fi
/usr/bin/codesign --verify --deep --strict "$bundle"
[[ $(xcrun lipo -archs "$bundle/Contents/MacOS/ctxfab-server") == arm64 ]]
runtime="$bundle/Contents/Helpers/Context Fabricator Runtime.app/Contents/MacOS/llama-server"
"$runtime" --version > "$report/runtime.log" 2>&1
grep -Fq 'build 12359, commit b1b770d90' "$report/runtime.log"
shasum -a 256 "$runtime" > "$report/runtime-fingerprint.txt"
shasum -a 256 "$bundle/Contents/MacOS/ctxfab-server" "$bundle/Contents/MacOS/ctxfab" > "$report/helper-fingerprints.txt"
python3 -B "$quality_root/quality/performance/workload.py" "$bundle" "$quality_root" "$assets" "$report"
echo "Mixed workload (${CTXFAB_M11_WORKLOAD_MODE:-controlled}) passed."
