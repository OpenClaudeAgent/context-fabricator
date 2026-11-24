#!/bin/bash
set -euo pipefail
shopt -s globstar nullglob

if [[ $# != 5 ]]; then
    echo "Usage: collect-candidate.sh MACOS_ARTIFACT LINUX_ARTIFACT OUTPUT VERSION DEBIAN_VERSION" >&2
    exit 2
fi
macos=$1 linux=$2 output=$3 version=$4 debian_version=$5
manifests=("$macos"/**/manifest.json)
packages=("$linux"/**/*.deb)
if [[ ${#manifests[@]} != 1 || ${#packages[@]} != 1 ]]; then
    echo "Expected one macOS package manifest and one Debian package." >&2
    exit 1
fi

manifest=${manifests[0]}
archive_name=$(jq -er --arg version "$version" '
    select(.version == $version and .signature == "adHoc" and .notarized == false)
    | .archive | select(test("^context-fabricator-[0-9.]+-[0-9.]+-macos-arm64-local\\.zip$"))
' "$manifest")
archive="${manifest%/*}/$archive_name"
digest=$(jq -er '.archiveSHA256 | select(test("^[a-f0-9]{64}$"))' "$manifest")
printf '%s  %s\n' "$digest" "$archive" | sha256sum -c -

deb=${packages[0]}
if [[ ${deb##*/} != "context-fabricator-qt_${debian_version}_amd64.deb" ||
      $(dpkg-deb --field "$deb" Package) != context-fabricator-qt ||
      $(dpkg-deb --field "$deb" Architecture) != amd64 ||
      $(dpkg-deb --field "$deb" Version) != "$debian_version" ]]; then
    echo "Unexpected Debian package identity, architecture or version." >&2
    exit 1
fi

mkdir "$output"
cp "$archive" "$deb" "$output/"
(
    cd "$output"
    sha256sum -- "$archive_name" "${deb##*/}" > SHA256SUMS
)
