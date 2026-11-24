#!/bin/bash
# Validate staged Developer ID and notarized bundles before clean-Mac acceptance.
set -euo pipefail

if [[ $# != 2 && $# != 3 ]]; then
    echo "Usage: check-distribution.sh APP_BUNDLE [RUNTIME_BUNDLE] APPLE_TEAM_ID" >&2
    exit 2
fi
app=$1
team=${!#}
if [[ ! $team =~ ^[A-Z0-9]{10}$ ]]; then
    echo "Usage: check-distribution.sh APP_BUNDLE [RUNTIME_BUNDLE] APPLE_TEAM_ID" >&2
    exit 2
fi
developer_id="anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"$team\""

verify_identity() {
    local path=$1 identifier=${2:-} requirement=$developer_id
    if [[ -n $identifier ]]; then
        requirement="$requirement and identifier \"$identifier\""
    fi
    /usr/bin/codesign --verify --strict --all-architectures -R="$requirement" "$path"
}

verify_bundle() {
    local bundle=$1 identifier=$2 member
    verify_identity "$bundle" "$identifier"
    /usr/bin/codesign --verify --strict --deep --all-architectures "$bundle"
    while IFS= read -r -d '' member; do
        if /usr/bin/file -b "$member" | /usr/bin/grep -Fq Mach-O; then
            verify_identity "$member"
            [[ $(xcrun lipo -archs "$member") == arm64 ]]
        fi
    done < <(/usr/bin/find "$bundle/Contents" -type f -print0)
    xcrun stapler validate "$bundle"
    /usr/sbin/spctl --assess --type execute --verbose=4 "$bundle"
}

python3 -B "$(dirname "$0")/app_distribution.py" verify "$app" --team-id "$team"
if [[ $# == 3 ]]; then
    runtime=$2
    verify_bundle "$runtime" org.contextfabricator.runtime.llama
    python3 -B "$(dirname "$0")/runtime_source.py" "$runtime"
fi
