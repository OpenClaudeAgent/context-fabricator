#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/environment.sh"

require_mac
quality_use_managed_rust
mkdir -p "$tools_dir/bin"
downloads=$(mktemp -d "$tools_dir/downloads.XXXXXX")
trap 'rm -rf -- "$downloads"' EXIT

download() {
    local url=$1 checksum=$2 output=$3
    curl --fail --silent --show-error --location --retry 3 --output "$output" "$url"
    printf '%s  %s\n' "$checksum" "$output" | shasum -a 256 --check
}

if [[ ! -x "$CARGO_HOME/bin/rustup" ]]; then
    download "https://static.rust-lang.org/rustup/archive/$rustup_version/aarch64-apple-darwin/rustup-init" \
        63752a1919a3a4793173ec70f8f212621a7ce38ba6a246095984482f4de20f9f "$downloads/rustup-init"
    chmod +x "$downloads/rustup-init"
    "$downloads/rustup-init" -y --no-modify-path --default-toolchain none
fi
rustup set auto-self-update disable
cd "$quality_root"
# Rustup reads the channel, profile, and components from rust-toolchain.toml.
rustup toolchain install --no-self-update

if [[ $(swiftlint version 2>/dev/null || true) != "$swiftlint_version" || ! -x "$tools_dir/bin/swiftlint" ]]; then
    download "https://github.com/realm/SwiftLint/releases/download/$swiftlint_version/portable_swiftlint.zip" \
        c1e429b0599cf1b516f369a2d9ec04eaf0e436f3c12b637df8851fa52ff694d0 "$downloads/swiftlint.zip"
    unzip -qjo "$downloads/swiftlint.zip" swiftlint -d "$downloads"
    mv "$downloads/swiftlint" "$tools_dir/bin/"
fi

if [[ $(cargo-deny --version 2>/dev/null || true) != "cargo-deny $cargo_deny_version" || ! -x "$tools_dir/bin/cargo-deny" ]]; then
    download "https://github.com/EmbarkStudios/cargo-deny/releases/download/$cargo_deny_version/cargo-deny-$cargo_deny_version-aarch64-apple-darwin.tar.gz" \
        c99a789160775ada8cb54aed72cfdae5a9b72e27434f8868d17c9634c3f52425 "$downloads/deny.tar.gz"
    tar -xOf "$downloads/deny.tar.gz" "cargo-deny-$cargo_deny_version-aarch64-apple-darwin/cargo-deny" > "$downloads/cargo-deny"
    chmod +x "$downloads/cargo-deny"
    mv "$downloads/cargo-deny" "$tools_dir/bin/"
fi

if [[ $(cargo-llvm-cov llvm-cov --version 2>/dev/null || true) != "cargo-llvm-cov $cargo_llvm_cov_version" || ! -x "$tools_dir/bin/cargo-llvm-cov" ]]; then
    download "https://github.com/taiki-e/cargo-llvm-cov/releases/download/v$cargo_llvm_cov_version/cargo-llvm-cov-aarch64-apple-darwin.tar.gz" \
        7e821a6c90c0884f79f759f5d9be50799ade67bb82515186d922f26032ef43b8 "$downloads/llvm-cov.tar.gz"
    tar -xzf "$downloads/llvm-cov.tar.gz" -C "$downloads" cargo-llvm-cov
    mv "$downloads/cargo-llvm-cov" "$tools_dir/bin/"
fi
