#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/toolchain.sh"

require_mac
cd "$quality_root/desktop/macos"
"$tools_dir/xcodegen/xcodegen/bin/xcodegen" generate --spec project.yml
mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/analysis.XXXXXX")
build=$(mktemp -d "$quality_dir/analysis-build.XXXXXX")
trap 'rm -rf -- "$build"' EXIT
echo "Swift analysis reports in $reports"
if ! xcodebuild -project ContextFabricator.xcodeproj -scheme ContextFabricator \
    -destination 'platform=macOS,arch=arm64' -onlyUsePackageVersionsFromResolvedFile \
    -skipMacroValidation -skipPackagePluginValidation -enableCodeCoverage NO \
    -derivedDataPath "$build" -clonedSourcePackagesDirPath "$quality_dir/foundation-packages" \
    build-for-testing > "$reports/compiler.log" 2>&1; then
    tail -n 80 "$reports/compiler.log"
    exit 1
fi

# Preserve the original compiler log and unescape the Xcode macro executable separator.
sed 's/\\#/#/g' "$reports/compiler.log" > "$reports/analyzer-input.log"
swiftlint analyze --strict --config .swiftlint.yml --compiler-log-path "$reports/analyzer-input.log" \
    App UITests Client/Sources 2>&1 | tee "$reports/swiftlint-analyze.log"
files=$(find App UITests Client/Sources -type f -name '*.swift' | wc -l | tr -d ' ')
grep -Eq "Found 0 violations, 0 serious in $files files\\.$" "$reports/swiftlint-analyze.log"
if grep -Ei '(^|: )(error:|warning:)|LLVM Profile Error' "$reports/swiftlint-analyze.log"; then
    exit 1
fi
