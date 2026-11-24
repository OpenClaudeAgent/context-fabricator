#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"
require_mac

bundle=${CTXFAB_M11_APP_BUNDLE:-"$quality_build_dir/release-xcode/Build/Products/Release/Context Fabricator.app"}
quality_begin_report report release-package
echo "Local release package reports in $report"
python3 -B "$quality_root/quality/release/app_distribution.py" stage "$bundle" "$report/package" \
    | tee "$report/notices.log"
args=("$report/package" "$report")
if [[ -n ${CTXFAB_M11_PREVIOUS_PACKAGE:-} ]]; then
    args+=(--previous "$CTXFAB_M11_PREVIOUS_PACKAGE")
fi
python3 -B "$quality_root/quality/release/release.py" "${args[@]}"

if [[ -n ${CTXFAB_RELEASE_PACKAGE_DIR:-} ]]; then
    python3 -B - "$report/package" "$CTXFAB_RELEASE_PACKAGE_DIR" "$quality_dir" <<'PYTHON'
from pathlib import Path
import shutil
import sys

source, output, quality = map(Path, sys.argv[1:])
if output.resolve().is_relative_to(quality.resolve()) or output.exists() or output.is_symlink():
    raise SystemExit("Select a new release package path outside .quality.")
output.parent.mkdir(parents=True, exist_ok=True)
shutil.copytree(source, output)
print(f"Release package exported to {output.resolve()}")
PYTHON
fi
