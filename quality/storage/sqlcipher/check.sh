#!/bin/bash
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=../../toolchain/environment.sh
source "$(dirname "$0")/../../toolchain/environment.sh"
require_mac

manifest="$quality_root/quality/storage/sqlcipher/Cargo.toml"
work=$(quality_workspace sqlcipher)
quality_on_exit 'rm -rf -- "$work"'
export CARGO_TARGET_DIR="$work"
# The arm64 qualification pins the native Apple crypto provider.
unset OPENSSL_DIR OPENSSL_LIB_DIR OPENSSL_INCLUDE_DIR
unset AARCH64_APPLE_DARWIN_OPENSSL_DIR AARCH64_APPLE_DARWIN_OPENSSL_LIB_DIR
unset AARCH64_APPLE_DARWIN_OPENSSL_INCLUDE_DIR LIBSQLITE3_FLAGS

mkdir -p "$quality_dir/reports"
quality_begin_report report storage-protection
echo "Storage protection qualification reports in $report"

cargo fmt --manifest-path "$manifest" --all -- --check
cargo clippy --locked --manifest-path "$manifest" --all-targets -- -D warnings > "$report/clippy.log" 2>&1
cargo test --locked --manifest-path "$manifest" --release --test round_trip -- --nocapture \
    2>&1 | tee "$report/sqlcipher.log"

fixture=$(
  cargo test --locked --manifest-path "$manifest" --release --test round_trip --no-run \
      --message-format=json 2> "$report/target.log" |
      jq -er 'select(.reason == "compiler-artifact" and .target.name == "round_trip" and .executable) | .executable'
)
xcrun lipo -archs "$fixture" > "$report/architecture.log"
[[ $(< "$report/architecture.log") == arm64 ]]
xcrun otool -L "$fixture" > "$report/linkage.log"
xcrun nm -u "$fixture" | awk '/_CCCryptor|_CCHmac/{print}' > "$report/commoncrypto.log"
test -s "$report/commoncrypto.log"
if rg -qi 'libcrypto|libtomcrypt' "$report/linkage.log"; then
    echo "The fixture links an unexpected crypto library." >&2
    exit 1
fi

cargo metadata --locked --manifest-path "$manifest" --format-version 1 |
    jq '[.packages[] | select(.name == "rusqlite" or .name == "libsqlite3-sys" or .name == "sqlcipher-src") |
          {name, version, license, manifest_path}]' > "$report/provider-packages.json"
cipher_source=$(jq -er '.[] | select(.name == "sqlcipher-src") | .manifest_path' "$report/provider-packages.json")
(
    cd "$(dirname "$cipher_source")"
    shasum -a 256 LICENSE sqlcipher/LICENSE-sqlcipher sqlcipher/LICENSE-libtomcrypt
) > "$report/source-notices.log"
printf 'SQLCipher version, provider linkage and source notices verified. Reports in %s\n' "$report"
