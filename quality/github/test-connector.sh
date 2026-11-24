#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"
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
quality_begin_report reports github-connector
work=$(quality_workspace github-connector-work)
quality_on_exit 'rm -rf -- "$work"'
source_app="$quality_build_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
app="$work/Context Fabricator.app"
helper="$app/Contents/MacOS/ctxfab-server"
sign_fixture() {
    local source=$1 target=$2
    /usr/bin/codesign --verify --deep --strict "$source"
    ditto "$source" "$target"
    /usr/bin/codesign --force --sign "$identity" --timestamp=none \
        --identifier org.contextfabricator.backend "$target/Contents/MacOS/ctxfab-server"
    /usr/bin/codesign --force --sign "$identity" --timestamp=none \
        --preserve-metadata=identifier,entitlements,flags,runtime "$target"
    /usr/bin/codesign --verify --deep --strict "$target"
}

upgrade_source=${CTXFAB_M07_UPGRADE_BUNDLE:-}
initial_source=${CTXFAB_M07_BASE_BUNDLE:-$source_app}
sign_fixture "$initial_source" "$app"
/usr/bin/codesign -dv "$helper" 2>&1 | awk '/^Identifier=|^Signature size=/ { print }' > "$reports/signature.txt"
upgraded_helper="$helper"
if [[ -n $upgrade_source ]]; then
    upgraded_app="$work/upgrade/Context Fabricator.app"
    upgraded_helper="$upgraded_app/Contents/MacOS/ctxfab-server"
    if cmp -s "$initial_source/Contents/MacOS/ctxfab-server" \
        "$upgrade_source/Contents/MacOS/ctxfab-server"; then
        echo "The upgrade fixture requires two distinct signed helpers." >&2
        exit 1
    fi
    mkdir "$work/upgrade"
    sign_fixture "$upgrade_source" "$upgraded_app"
    shasum -a 256 < "$helper" | awk '{ print "initial " $1 }' > "$reports/upgrade-helpers.txt"
    shasum -a 256 < "$upgraded_helper" | awk '{ print "replacement " $1 }' >> "$reports/upgrade-helpers.txt"
fi

echo "Signed GitHub connector fixture reports in $reports"
python3 "$quality_root/quality/github/package_check.py" "$helper" "$work" "$upgraded_helper" \
    | tee "$reports/keychain.log"
