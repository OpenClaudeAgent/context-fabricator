#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"
source "$quality_root/quality/answers/profile.sh"
source "$quality_root/quality/retrieval/profile.sh"
require_mac

if [[ $# -gt 1 ]]; then
    echo "GitHub answer qualification accepts at most one mode argument." >&2
    exit 2
fi
mkdir -p "$quality_dir/reports"
default_bundle="$quality_build_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
case "${1:-}" in
    --hosted-preflight|--hosted)
        hosted=$1
        default_bundle="$quality_build_dir/release-xcode/Build/Products/Release/Context Fabricator.app"
        quality_begin_report report github-hosted
        ;;
    "")
        hosted=
        quality_begin_report report github-answers
        ;;
    *)
        echo "Usage: bash quality/github/qualify-answers.sh [--hosted-preflight | --hosted]" >&2
        exit 2
        ;;
esac
echo "GitHub answer qualification reports in $report"
if [[ $hosted == --hosted-preflight ]]; then
    python3 -B "$quality_root/quality/github/hosted.py" "$quality_root" "$report"
    exit 0
fi

bundle=${CTXFAB_M10_APP_BUNDLE:-"$default_bundle"}
/usr/bin/codesign --verify --deep --strict "$bundle"
if [[ $hosted == --hosted ]]; then
    python3 -B "$quality_root/quality/github/hosted.py" "$quality_root" "$report" --bundle "$bundle"
    echo "Hosted GitHub answer qualification passed."
    exit 0
fi
assets=${CTXFAB_M10_ASSETS:-"$quality_cache_dir/m09-assets"}
(cd "$assets" && printf '%s  %s\n' "$m09_sha256" "$m09_model" | shasum -a 256 -c) \
    | tee "$report/model-integrity.log"
if [[ ${CTXFAB_M10_EVIDENCE_ONLY:-} == hybrid ]]; then
    (cd "$assets" && printf '%s  %s\n' "$m08_sha256" "$m08_model" | shasum -a 256 -c) \
        | tee "$report/embedding-integrity.log"
fi
engine="$bundle/Contents/Helpers/Context Fabricator Runtime.app/Contents/MacOS/llama-server"
runtime=$("$engine" --version 2>&1)
printf '%s\n' "$runtime" | tee "$report/runtime.log"
grep -Fq 'build 12359, commit b1b770d90' <<< "$runtime"
python3 -B "$quality_root/quality/github/dogfood.py" "$bundle" "$quality_root" "$assets" "$report"
echo "GitHub answer qualification passed."
