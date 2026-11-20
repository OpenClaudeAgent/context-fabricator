#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"

require_mac
cd "$quality_root/desktop/macos"
"$tools_dir/xcodegen/xcodegen/bin/xcodegen" generate --spec project.yml
mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/runtime.XXXXXX")
echo "Runtime qualification reports in $reports"

smoke="$quality_root/quality/integration/smoke-app.sh"
app="$quality_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
/bin/bash "$smoke" "$app" main-thread | tee "$reports/main-thread.log"

for diagnostic in address thread; do
    case "$diagnostic" in
        address) flag=-enableAddressSanitizer; symbol=__asan_ ;;
        thread) flag=-enableThreadSanitizer; symbol=__tsan_ ;;
    esac
    derived="$quality_dir/m02-$diagnostic-xcode"
    if ! xcodebuild -project ContextFabricator.xcodeproj -scheme ContextFabricator \
        -configuration Debug -destination 'platform=macOS,arch=arm64' \
        -onlyUsePackageVersionsFromResolvedFile -skipMacroValidation -skipPackagePluginValidation \
        -derivedDataPath "$derived" -clonedSourcePackagesDirPath "$quality_dir/foundation-packages" \
        -enableCodeCoverage NO "$flag" YES build-for-testing > "$reports/$diagnostic-build.log" 2>&1; then
        tail -n 60 "$reports/$diagnostic-build.log" >&2
        exit 1
    fi
    app="$derived/Build/Products/Debug/Context Fabricator.app"
    codesign --force --sign - --timestamp=none \
        --preserve-metadata=identifier,entitlements,flags,runtime "$app"
    xcrun nm -u "$app/Contents/MacOS/Context Fabricator.debug.dylib" > "$reports/$diagnostic-symbols.log"
    grep -F "$symbol" "$reports/$diagnostic-symbols.log" > /dev/null
    /bin/bash "$smoke" "$app" "$diagnostic" | tee "$reports/$diagnostic.log"
done
echo "Passed Main Thread Checker, Address Sanitizer and Thread Sanitizer. Reports in $reports"
