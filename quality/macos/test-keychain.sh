#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

require_mac
identity=${CTXFAB_M03_SIGNING_IDENTITY:-}
if [[ -z $identity ]]; then
    identities=$(/usr/bin/security find-identity -v -p codesigning |
        awk 'length($2) == 40 && /"Apple Development:/ { print $2 }')
    if [[ ! $identities =~ ^[[:xdigit:]]{40}$ ]]; then
        echo "Keychain qualification requires one Apple Development signing identity or CTXFAB_M03_SIGNING_IDENTITY." >&2
        exit 1
    fi
    identity=$identities
fi

mkdir -p "$quality_dir/reports"
quality_begin_report reports keychain
work=$(quality_workspace keychain-work)
source_app="$quality_build_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
app="$work/Context Fabricator.app"
data="$work/data"
correlation=$(uuidgen | tr '[:upper:]' '[:lower:]')
launcher="$app/Contents/MacOS/Context Fabricator"
helper="$app/Contents/MacOS/ctxfab-server"
cli="$app/Contents/MacOS/ctxfab"
app_pid=
endpoint="$data/endpoint.json"
fixture_present=0

stop() {
    if [[ -z $app_pid ]]; then return; fi
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
    app_pid=
    for ((attempt = 0; attempt < 50; attempt++)); do
        if [[ ! -e $endpoint ]]; then return; fi
        sleep 0.1
    done
    echo "The signed backend did not withdraw discovery on shutdown." >&2
    return 1
}

cleanup() {
    stop || true
    if [[ $fixture_present == 1 ]]; then
        if ! /usr/bin/security delete-generic-password -s org.contextfabricator.providers \
            -a "fixture:$correlation" > /dev/null 2>&1; then
            echo "Check cleanup of disposable Keychain reference fixture:$correlation." >&2
        fi
    fi
    rm -rf -- "$work"
}
quality_on_exit cleanup

ditto "$source_app" "$app"
/usr/bin/codesign --force --sign "$identity" --timestamp=none \
    --identifier org.contextfabricator.backend "$helper"
/usr/bin/codesign --force --sign "$identity" --timestamp=none \
    --preserve-metadata=identifier,entitlements,flags,runtime "$app"
/usr/bin/codesign --verify --deep --strict "$app"
/usr/bin/codesign -dv "$helper" 2>&1 | awk '/^Identifier=|^Signature size=/ { print }' > "$reports/signature.txt"
grep -Fxq 'Identifier=org.contextfabricator.backend' "$reports/signature.txt"
grep -q '^Signature size=' "$reports/signature.txt"

wait_for_endpoint() {
    for ((attempt = 0; attempt < 100; attempt++)); do
        if [[ -s $endpoint ]]; then return; fi
        kill -0 "$app_pid" 2>/dev/null || break
        sleep 0.1
    done
    echo "The backend failed to publish its endpoint. Reports in $reports" >&2
    return 1
}

launch() {
    local phase=$1
    env CTXFAB_TEST_DATA_DIRECTORY="$data" CTXFAB_TEST_CREDENTIAL_FIXTURE="--fixture-$phase" \
        CTXFAB_TEST_CREDENTIAL_CORRELATION="$correlation" \
        LLVM_PROFILE_FILE="$work/launcher-%p.profraw" \
        "$launcher" > "$reports/$phase.log" 2>&1 &
    app_pid=$!
    wait_for_endpoint
}

diagnostics() {
    CTXFAB_DATA_DIRECTORY="$data" "$cli" diagnostics | tee "$reports/$1.json"
}

echo "Signed Keychain reports in $reports"
fixture_present=1
launch store
diagnostics stored |
    jq -e --arg id "$correlation" 'any(.[]; .code == "fixture_credential_stored" and (.message | contains($id)))' > /dev/null
stop

LLVM_PROFILE_FILE="$work/helper-%p.profraw" \
    "$source_app/Contents/MacOS/ctxfab-server" "$data" --fixture-rotate "$correlation" \
    > "$reports/ad-hoc.log" 2>&1 &
app_pid=$!
wait_for_endpoint
diagnostics denied |
    jq -e --arg id "$correlation" '
        any(.[]; .code == "fixture_credential_failed"
          and (.message | contains("Keychain denied fixture credential access."))
          and (.message | contains($id)))
    ' > /dev/null
stop

launch rotate
diagnostics rotated |
    jq -e --arg id "$correlation" '
        [.[] | select(.message | contains($id)) | .code]
        == ["fixture_credential_rotated", "fixture_credential_failed", "fixture_credential_stored"]
    ' > /dev/null
fixture_present=0
stop
echo "Signed helper created, read, rotated and removed a disposable Keychain item across two app launches."
echo "The ad hoc helper was denied access to the same item."
