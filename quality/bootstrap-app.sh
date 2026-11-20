#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/toolchain.sh"

require_mac
mkdir -p "$tools_dir/bin" "$tools_dir/xcodegen" "$tools_dir/openapi"
downloads=$(mktemp -d "$tools_dir/app-downloads.XXXXXX")
trap 'rm -rf -- "$downloads"' EXIT

download() {
    local url=$1 checksum=$2 output=$3
    curl --fail --silent --show-error --location --retry 3 --output "$output" "$url"
    printf '%s  %s\n' "$checksum" "$output" | shasum -a 256 --check
}

if [[ "$("$tools_dir/xcodegen/xcodegen/bin/xcodegen" --version 2>/dev/null || true)" != "Version: 2.47.0" ]]; then
    download https://github.com/yonaskolb/XcodeGen/releases/download/2.47.0/xcodegen.zip \
        6ef12a3ef5327c2c713397f46d9160550f8b1cdc5c925cd884d65c77c9f696cd "$downloads/xcodegen.zip"
    unzip -qo "$downloads/xcodegen.zip" -d "$tools_dir/xcodegen"
fi
if [[ ! -f "$tools_dir/openapi/generator.jar" ]]; then
    download https://repo.maven.apache.org/maven2/org/openapitools/openapi-generator-cli/7.27.0/openapi-generator-cli-7.27.0.jar \
        db9c4b738727698748746144394cb7ee96512fe50166555baaeeca9311ee1dba "$downloads/generator.jar"
    mv "$downloads/generator.jar" "$tools_dir/openapi/"
fi
if [[ ! -x "$tools_dir/bin/oasdiff" ]]; then
    download https://github.com/oasdiff/oasdiff/releases/download/v1.39.0/oasdiff_1.39.0_darwin_all.tar.gz \
        fe88d9950324d3b0ebeaf8e0d3a210b64cb099a0f55fce5ee5a22b276df0eac4 "$downloads/oasdiff.tar.gz"
    tar -xzf "$downloads/oasdiff.tar.gz" -C "$downloads" oasdiff
    mv "$downloads/oasdiff" "$tools_dir/bin/"
fi
if [[ ! -x "$tools_dir/bin/toxiproxy-server" ]]; then
    download https://github.com/Shopify/toxiproxy/releases/download/v2.13.0/toxiproxy-server-darwin-arm64 \
        21e5d0cbfc4af6e9967f617ee7a1645fa26181220fee3961630311a8de14705c "$downloads/toxiproxy-server"
    chmod +x "$downloads/toxiproxy-server"
    mv "$downloads/toxiproxy-server" "$tools_dir/bin/"
fi
