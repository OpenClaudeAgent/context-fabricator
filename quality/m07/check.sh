#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"
require_mac

identity=${CTXFAB_M07_SIGNING_IDENTITY:-}
if [[ -z $identity ]]; then
    identity=$(/usr/bin/security find-identity -v -p codesigning |
        awk 'length($2) == 40 && /"Apple Development:/ { print $2 }')
    if [[ ! $identity =~ ^[[:xdigit:]]{40}$ ]]; then
        echo "Set CTXFAB_M07_SIGNING_IDENTITY to an Apple Development identity." >&2
        exit 1
    fi
fi

mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/m07.XXXXXX")
work=$(mktemp -d "$quality_dir/m07.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
source_app="$quality_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
app="$work/Context Fabricator.app"
helper="$app/Contents/MacOS/ctxfab-server"
ditto "$source_app" "$app"
/usr/bin/codesign --force --sign "$identity" --timestamp=none \
    --identifier org.contextfabricator.backend "$helper"
/usr/bin/codesign --force --sign "$identity" --timestamp=none \
    --preserve-metadata=identifier,entitlements,flags,runtime "$app"
/usr/bin/codesign --verify --deep --strict "$app"
/usr/bin/codesign -dv "$helper" 2>&1 | awk '/^Identifier=|^Signature size=/ { print }' > "$reports/signature.txt"

echo "Signed M07 fixture reports in $reports"
python3 "$quality_root/quality/m07/package_check.py" "$helper" "$work" | tee "$reports/keychain.log"
