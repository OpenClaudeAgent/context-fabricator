#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

require_mac
cd "$quality_root/desktop/macos"
"$tools_dir/xcodegen/xcodegen/bin/xcodegen" generate --spec project.yml
mkdir -p "$quality_dir/reports"
quality_begin_report reports analysis
actions=(build-for-testing)
if [[ -n ${CI:-} ]]; then
    build="$quality_build_dir/foundation-xcode"
    actions=(clean build-for-testing)
else
    build=$(quality_workspace analysis-build)
    quality_on_exit 'rm -rf -- "$build"'
fi
echo "Swift analysis reports in $reports"
if ! xcodebuild -project ContextFabricator.xcodeproj -scheme ContextFabricator \
    -destination 'platform=macOS,arch=arm64' -onlyUsePackageVersionsFromResolvedFile \
    -skipMacroValidation -skipPackagePluginValidation -enableCodeCoverage NO \
    -derivedDataPath "$build" -clonedSourcePackagesDirPath "$quality_build_dir/foundation-packages" \
    "${actions[@]}" 2>&1 |
    sed '/^[[:space:]]*error: The command /d' > "$reports/compiler.log"; then
    grep -E -m 20 '(^xcodebuild: error:|:[0-9]+:[0-9]+: error: )' "$reports/compiler.log" || true
    echo "Xcode build failed. Compiler report in $reports/compiler.log" >&2
    exit 1
fi

# SwiftPM product warnings can come from handwritten, generated, or dependency sources.
awk -v root="$PWD/" '
    /:[0-9]+:[0-9]+: warning: |^warning: / && !seen[$0]++ {
        if (index($0, root "App/") == 1 || index($0, root "UITests/") == 1 ||
            index($0, root "Client/Sources/") == 1 || index($0, root "Client/Tests/") == 1) {
            print "first-party: " $0
            failures++
        } else if ($0 ~ /\/OpenAPIGenerator\/GeneratedSources\//) {
            print "generated: " $0
        } else {
            print "other: " $0
        }
    }
    END { if (failures) exit 1 }
' "$reports/compiler.log" | tee "$reports/compiler-warnings.log"

# Keep the compiler report and unescape the Xcode macro executable separator.
sed 's/\\#/#/g' "$reports/compiler.log" > "$reports/analyzer-input.log"
swiftlint analyze --strict --config .swiftlint.yml --compiler-log-path "$reports/analyzer-input.log" \
    App UITests Client/Sources 2>&1 | tee "$reports/swiftlint-analyze.log"
files=$(find App UITests Client/Sources -type f -name '*.swift' | wc -l | tr -d ' ')
grep -Eq "Found 0 violations, 0 serious in $files files\\.$" "$reports/swiftlint-analyze.log"
if grep -Ei '(^|: )(error:|warning:)|LLVM Profile Error' "$reports/swiftlint-analyze.log"; then
    exit 1
fi
