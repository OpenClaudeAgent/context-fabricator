#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/toolchain.sh"

require_mac
cd "$quality_root/desktop/macos"
check_app_coverage() {
    jq -er --arg app "$PWD/App/" --argjson minimum "$minimum_line_coverage" '
        [.targets[].files[] | select(.path | startswith($app))]
        | unique_by(.path)
        | (map(.coveredLines) | add // 0) as $covered
        | (map(.executableLines) | add // 0) as $total
        | select($total > 0)
        | "\($covered)/\($total) native Swift app lines, minimum \($minimum)%",
          ($covered * 100 >= $minimum * $total)
        ' "$1"
}

if [[ ${1:-} == merge-ui ]]; then
    mkdir -p "$quality_dir/reports"
    report=$(mktemp -d "$quality_dir/reports/app-coverage.XXXXXX")
    inputs=()
    for shard in foundation answers github; do
        shard_dir="$2/native-ui-$shard"
        pairs=("$shard_dir"/app.*/coverage/*.xccovreport "$shard_dir"/coverage/*.xccovreport)
        found=0
        for coverage in "${pairs[@]}"; do
            [[ -f $coverage ]] || continue
            archive=${coverage%.xccovreport}.xccovarchive
            [[ -d $archive ]]
            inputs+=("$coverage" "$archive")
            found=$((found + 1))
        done
        [[ $found == 1 ]] || { echo "Expected one coverage pair for $shard." >&2; exit 1; }
    done
    xcrun xccov merge --outReport "$report/merged.xccovreport" \
        --outArchive "$report/merged.xccovarchive" "${inputs[@]}"
    xcrun xccov view --report --json "$report/merged.xccovreport" > "$report/coverage.json"
    check_app_coverage "$report/coverage.json" | tee "$report/coverage-gate.log"
    echo "Merged native app coverage in $report"
    exit 0
fi

if [[ ${1:-} == unit ]]; then
    mkdir -p "$quality_dir/reports"
    report=$(mktemp -d "$quality_dir/reports/swift-unit.XXXXXX")
    echo "Swift unit reports in $report"
    cargo build --manifest-path "$quality_root/backend/Cargo.toml" --locked -p ctxfab-server
    CTXFAB_TEST_BACKEND_EXECUTABLE="$quality_root/backend/target/debug/ctxfab-server" \
        xcrun swift test --package-path Client --only-use-versions-from-resolved-file \
        --enable-code-coverage --xunit-output "$report/tests.xml" 2>&1 | tee "$report/test.log"
    cp "$(xcrun swift test --package-path Client --show-codecov-path)" "$report/coverage.json"
    grep -Eq 'Test run with [1-9][0-9]* tests .* passed' "$report/test.log"
    grep -Fq 'Test generatedClientReadsAndUpdatesTheLiveRustService() passed' "$report/test.log"
    jq -er --arg sources "$PWD/Client/Sources/" --argjson minimum "$minimum_line_coverage" '
        [.data[].files[] | select(.filename | startswith($sources))]
        | unique_by(.filename)
        | (map(.summary.lines.covered) | add // 0) as $covered
        | (map(.summary.lines.count) | add // 0) as $total
        | select($total > 0)
        | "\($covered)/\($total) first-party desktop package lines, minimum \($minimum)%",
          ($covered * 100 >= $minimum * $total)
        ' "$report/coverage.json" | tee "$report/coverage-gate.log"
    exit 0
fi
"$tools_dir/xcodegen/xcodegen/bin/xcodegen" generate --spec project.yml
xcode=(xcodebuild -project ContextFabricator.xcodeproj -scheme ContextFabricator
    -destination 'platform=macOS,arch=arm64' -onlyUsePackageVersionsFromResolvedFile
    -skipMacroValidation -skipPackagePluginValidation
    -derivedDataPath "$quality_dir/foundation-xcode"
    -clonedSourcePackagesDirPath "$quality_dir/foundation-packages")
if [[ ${1:-} == test ]]; then
    shard=${CTXFAB_UI_SHARD:-all}
    suite=ContextFabricatorUITests/FoundationUITests
    case "$shard" in
        all) subjects=(-skip-testing:"$suite/testLaunchToBackendReadyPerformance") ;;
        foundation) subjects=(
            -skip-testing:ContextFabricatorUITests/GitHubUITests
            -skip-testing:"$suite/testLaunchToBackendReadyPerformance"
            -skip-testing:"$suite/testAnswersSwitchesModelWithoutRestartingAndShowsInsufficientEvidence"
            -skip-testing:"$suite/testAnswersCancelsAnInFlightGenerationAndAcceptsTheNextQuestion") ;;
        answers) subjects=(
            -only-testing:"$suite/testAnswersSwitchesModelWithoutRestartingAndShowsInsufficientEvidence"
            -only-testing:"$suite/testAnswersCancelsAnInFlightGenerationAndAcceptsTheNextQuestion") ;;
        github) subjects=(-only-testing:ContextFabricatorUITests/GitHubUITests) ;;
        *) echo "Unknown macOS UI test shard: $shard" >&2; exit 2 ;;
    esac
    mkdir -p "$quality_dir/reports"
    report=$(mktemp -d "$quality_dir/reports/app.XXXXXX")
    echo "App reports in $report"
    if [[ $shard == all || $shard == github ]]; then
        fixture=$(mktemp -d "$quality_dir/github-ui.XXXXXX")
        CTXFAB_GITHUB_UI_ADVANCE=1 python3 "$quality_root/quality/m07/github_fixture.py" "$fixture" \
            > "$report/github-fixture.log" 2>&1 &
        fixture_pid=$!
        cleanup_fixture() {
            kill "$fixture_pid" 2>/dev/null || true
            wait "$fixture_pid" 2>/dev/null || true
            if [[ -f $fixture/hits.log ]]; then cp "$fixture/hits.log" "$report/github-hits.log"; fi
            rm -rf -- "$fixture"
        }
        trap cleanup_fixture EXIT
        for ((attempt = 0; attempt < 100; attempt++)); do
            [[ ! -s $fixture/port ]] || break
            kill -0 "$fixture_pid" 2>/dev/null || break
            sleep 0.05
        done
        if [[ ! -s $fixture/port ]]; then
            cat "$report/github-fixture.log" >&2
            echo "The GitHub UI fixture did not start." >&2
            exit 1
        fi
        export TEST_RUNNER_CTXFAB_GITHUB_TEST_PORT="$(< "$fixture/port")"
        export TEST_RUNNER_CTXFAB_GITHUB_TEST_HITS="$fixture/hits.log"
    fi
    "${xcode[@]}" -parallel-testing-enabled NO \
        "${subjects[@]}" -resultBundlePath "$report/App.xcresult" test |
        tee "$report/xcode.log"
    xcrun xcresulttool get test-results summary --path "$report/App.xcresult" > "$report/tests.json"
    jq -e '.passedTests > 0 and .failedTests == 0 and .skippedTests == 0' "$report/tests.json" > /dev/null
    xcrun xccov view --report "$report/App.xcresult" > "$report/coverage.txt"
    xcrun xccov view --report --json "$report/App.xcresult" > "$report/coverage.json"
    if [[ $shard == all ]]; then
        check_app_coverage "$report/coverage.json" | tee "$report/coverage-gate.log"
    else
        mkdir "$report/coverage"
        xcrun xcresulttool export coverage --path "$report/App.xcresult" --output-path "$report/coverage"
        mv "$report/coverage/"*_CoverageReport "$report/coverage/native.xccovreport"
        mv "$report/coverage/"*_CoverageArchive "$report/coverage/native.xccovarchive"
    fi
else
    "${xcode[@]}" build
    # Cargo can replace a nested executable on an otherwise incremental Xcode build.
    # Reseal the development bundle after the embed phase finishes.
    bundle="$quality_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
    /usr/bin/codesign --force --sign - --timestamp=none \
        --preserve-metadata=identifier,entitlements,flags,runtime "$bundle"
    /usr/bin/codesign --verify --deep --strict "$bundle"
fi
