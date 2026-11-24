#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

require_mac
cd "$quality_root/desktop/macos"
"$tools_dir/xcodegen/xcodegen/bin/xcodegen" generate --spec project.yml
mkdir -p "$quality_dir/reports"
quality_begin_report reports lifecycle-ui
echo "Lifecycle XCUI and performance reports in $reports"

work=$(quality_workspace lifecycle-ui)
quality_on_exit 'rm -rf -- "$work"'

xcode=(xcodebuild -project ContextFabricator.xcodeproj -scheme ContextFabricator
    -destination 'platform=macOS,arch=arm64' -onlyUsePackageVersionsFromResolvedFile
    -skipMacroValidation -skipPackagePluginValidation
    -derivedDataPath "$work"
    -clonedSourcePackagesDirPath "$quality_build_dir/foundation-packages"
    -parallel-testing-enabled NO -enableCodeCoverage NO)
suite=ContextFabricatorUITests/BackendLifecycleUITests
performance="$suite/testLaunchToBackendReadyPerformance"
if ! "${xcode[@]}" -only-testing:"$performance" -resultBundlePath "$reports/Performance.xcresult" test \
    > "$reports/performance.log" 2>&1; then
    tail -n 65 "$reports/performance.log" >&2
    exit 1
fi
xcrun xcresulttool get test-results summary --path "$reports/Performance.xcresult" > "$reports/performance-summary.json"
jq -e '.passedTests == 1 and .failedTests == 0' "$reports/performance-summary.json" > /dev/null
xcrun xcresulttool get test-results metrics --path "$reports/Performance.xcresult" > "$reports/performance-metrics.json"
jq -e --arg identifier "${performance#*/}()" '
    [.[] | select(.testIdentifier == $identifier)
     | .testRuns[].metrics[]
     | select(.identifier == "com.apple.dt.XCTMetric_Clock.time.monotonic" and .unitOfMeasurement == "s")
     | select(.measurements | length == 3 and all(.[]; . > 0))]
    | if length == 0 then error("No clock metric with three positive observations") else . end
' "$reports/performance-metrics.json"

status=0
TEST_RUNNER_CTXFAB_UI_PROBE_MISSING_BACKEND=1 "${xcode[@]}" \
    -only-testing:"$suite/testPackagedBackendBecomesReady" \
    -resultBundlePath "$reports/RejectedAssertion.xcresult" test \
    > "$reports/ui-failure.log" 2>&1 || status=$?
if [[ $status != 65 ]]; then
    tail -n 65 "$reports/ui-failure.log" >&2
    echo "The rejected UI assertion did not yield Xcode exit 65 (received $status)." >&2
    exit 1
fi
xcrun xcresulttool get test-results summary --path "$reports/RejectedAssertion.xcresult" > "$reports/ui-failure-summary.json"
jq -e '
    .failedTests == 1 and .passedTests == 0
    and any(.testFailures[];
        (.testName | contains("testPackagedBackendBecomesReady"))
        and (.failureText | contains("Expected the bundled backend to become ready.")))
' "$reports/ui-failure-summary.json" > /dev/null
xcrun xcresulttool export attachments --path "$reports/RejectedAssertion.xcresult" \
    --output-path "$reports/attachments" > "$reports/attachments.log"
attachment=$(jq -er '
    .[].attachments[]
    | select(.suggestedHumanReadableName | startswith("Backend startup accessibility state"))
    | .exportedFileName
' "$reports/attachments/manifest.json")
grep -F 'backend-unavailable' "$reports/attachments/$attachment" > /dev/null
printf 'XCUI rejected the deliberate assertion (Xcode exit %s); attachment shows backend-unavailable. Reports in %s\n' "$status" "$reports"
