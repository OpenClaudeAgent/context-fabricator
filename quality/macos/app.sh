#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

require_mac
cd "$quality_root/desktop/macos"
mode=${1:-build}
destination='platform=macOS,arch=arm64'
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
    quality_begin_report report app-coverage
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
    quality_begin_report report swift-unit
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
if [[ $mode != test-built-ui ]]; then
    "$tools_dir/xcodegen/xcodegen/bin/xcodegen" generate --spec project.yml
    [[ $mode != project ]] || exit 0
fi

configuration=Debug
derived_data="$quality_build_dir/foundation-xcode"
runtime_settings=()
if [[ ${1:-} == build-release ]]; then
    configuration=Release
    derived_data="$quality_build_dir/release-xcode"
    if [[ -n ${CTXFAB_RUNTIME_ARCHIVE_BYTES:-}${CTXFAB_RUNTIME_ARCHIVE_EXPANDED_BYTES:-}${CTXFAB_RUNTIME_BUNDLE_BYTES:-}${CTXFAB_RUNTIME_BUNDLE_MANIFEST_SHA256:-}${CTXFAB_RUNTIME_ARCHIVE_SHA256:-}${CTXFAB_RUNTIME_APPLE_TEAM_ID:-}${CTXFAB_RUNTIME_RELEASE_URL:-} ]]; then
        if [[ ! ${CTXFAB_RUNTIME_ARCHIVE_BYTES:-} =~ ^[1-9][0-9]{0,8}$ ||
            ! ${CTXFAB_RUNTIME_ARCHIVE_EXPANDED_BYTES:-} =~ ^[1-9][0-9]{0,9}$ ||
            ! ${CTXFAB_RUNTIME_BUNDLE_BYTES:-} =~ ^[1-9][0-9]{0,9}$ ||
            ! ${CTXFAB_RUNTIME_BUNDLE_MANIFEST_SHA256:-} =~ ^[0-9a-f]{64}$ ||
            ! ${CTXFAB_RUNTIME_ARCHIVE_SHA256:-} =~ ^[0-9a-f]{64}$ ||
            ! ${CTXFAB_RUNTIME_APPLE_TEAM_ID:-} =~ ^[A-Z0-9]{10}$ ]] ||
            (( CTXFAB_RUNTIME_ARCHIVE_BYTES > 512 * 1024 * 1024 ||
               CTXFAB_RUNTIME_ARCHIVE_EXPANDED_BYTES > 4 * 1024 * 1024 * 1024 ||
               CTXFAB_RUNTIME_BUNDLE_BYTES > CTXFAB_RUNTIME_ARCHIVE_EXPANDED_BYTES )); then
            echo "Expected archive and expanded sizes, bundle size and manifest SHA-256, archive SHA-256, and Apple Team ID for a managed runtime." >&2
            exit 2
        fi
        runtime_settings=(
            "CTXFAB_RUNTIME_ARCHIVE_BYTES=$CTXFAB_RUNTIME_ARCHIVE_BYTES"
            "CTXFAB_RUNTIME_ARCHIVE_EXPANDED_BYTES=$CTXFAB_RUNTIME_ARCHIVE_EXPANDED_BYTES"
            "CTXFAB_RUNTIME_BUNDLE_BYTES=$CTXFAB_RUNTIME_BUNDLE_BYTES"
            "CTXFAB_RUNTIME_BUNDLE_MANIFEST_SHA256=$CTXFAB_RUNTIME_BUNDLE_MANIFEST_SHA256"
            "CTXFAB_RUNTIME_ARCHIVE_SHA256=$CTXFAB_RUNTIME_ARCHIVE_SHA256"
            "CTXFAB_RUNTIME_APPLE_TEAM_ID=$CTXFAB_RUNTIME_APPLE_TEAM_ID"
        )
        if [[ -n ${CTXFAB_RUNTIME_RELEASE_URL:-} ]]; then
            runtime_settings+=("CTXFAB_RUNTIME_RELEASE_URL=$CTXFAB_RUNTIME_RELEASE_URL")
        fi
    fi
fi
xcode=(xcodebuild -project ContextFabricator.xcodeproj -scheme ContextFabricator
    -destination "$destination" -onlyUsePackageVersionsFromResolvedFile
    -skipMacroValidation -skipPackagePluginValidation
    -configuration "$configuration" -derivedDataPath "$derived_data"
    -clonedSourcePackagesDirPath "$quality_build_dir/foundation-packages")
if [[ $mode == build-ui-tests ]]; then
    archive=${2:?Expected an absolute path for the XCUI archive}
    [[ $archive = /* ]] || { echo 'Expected an absolute XCUI archive path.' >&2; exit 2; }
    "${xcode[@]}" -enableCodeCoverage YES build-for-testing
    products="$derived_data/Build/Products"
    sdk_version=$(xcrun --sdk macosx --show-sdk-version)
    plan="$products/ContextFabricator_macosx$sdk_version-arm64.xctestrun"
    [[ -f $plan ]] || {
        echo "Missing macOS arm64 test plan: $plan" >&2
        exit 1
    }
    staging=$(quality_workspace xcui-build)
    trap 'rm -rf -- "$staging"' EXIT
    staged="$staging/Products"
    mkdir -p "$staged/Debug"
    cp "$plan" "$staged/ContextFabricator.xctestrun"
    for bundle in 'Context Fabricator.app' ContextFabricatorUITests-Runner.app; do
        /usr/bin/codesign --verify --deep --strict "$products/Debug/$bundle"
        /usr/bin/ditto "$products/Debug/$bundle" "$staged/Debug/$bundle"
    done
    if [[ -d $products/Debug/PackageFrameworks ]]; then
        /usr/bin/ditto "$products/Debug/PackageFrameworks" "$staged/Debug/PackageFrameworks"
    fi
    /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$staged" "$archive"
    exit 0
fi

if [[ $mode == test || $mode == test-built-ui ]]; then
    shard=${CTXFAB_UI_SHARD:-all}
    target=ContextFabricatorUITests
    performance="$target/BackendLifecycleUITests/testLaunchToBackendReadyPerformance"
    case "$shard" in
        all) subjects=(-skip-testing:"$performance") ;;
        foundation) subjects=(
            -only-testing:"$target/BackendLifecycleUITests"
            -only-testing:"$target/DiagnosticsUITests"
            -skip-testing:"$performance") ;;
        answers) subjects=(
            -only-testing:"$target/AnswersUITests"
            -only-testing:"$target/ModelLibraryUITests") ;;
        models) subjects=(-only-testing:"$target/ModelLibraryUITests") ;;
        github) subjects=(-only-testing:"$target/GitHubUITests") ;;
        *) echo "Unknown macOS UI test shard: $shard" >&2; exit 2 ;;
    esac
    if [[ $shard == all || $shard == answers ]]; then
        if [[ ${CTXFAB_XCTEST_LIVE_BENCHMARK:-0} == 1 ]]; then
            export TEST_RUNNER_CTXFAB_XCTEST_LIVE_BENCHMARK=1
        else
            subjects+=(
                -skip-testing:"$target/AnswersUITests/testPinnedJacksonCorpusLoadsAndReplaysInInspector"
                -skip-testing:"$target/AnswersUITests/testCommonsCLIBenchmarkShowsRetrievedPassagesAndBM25Reason")
        fi
    fi
    mkdir -p "$quality_dir/reports"
    quality_begin_report report app
    echo "App reports in $report"
    temporary_base=${TMPDIR:-/tmp}
    fixture_root=$(mktemp -d "${temporary_base%/}/ctxfab-ui.XXXXXX")
    github_fixture=
    hub_fixture=
    github_pid=
    hub_pid=
    benchmark_pid=
    cleanup_fixtures() {
        local pid
        for pid in "$github_pid" "$hub_pid" "$benchmark_pid"; do
            [[ -n $pid ]] || continue
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        done
        if [[ -n $github_fixture ]]; then
            if [[ -f $github_fixture/hits.log ]]; then cp "$github_fixture/hits.log" "$report/github-hits.log"; fi
        fi
        if [[ -n $hub_fixture ]]; then cp -R "$hub_fixture" "$report/hugging-face"; fi
        rm -rf -- "$fixture_root"
    }
    quality_on_exit cleanup_fixtures

    wait_for_fixture() {
        local port_file=$1 pid=$2 log=$3 attempt
        for ((attempt = 0; attempt < 100; attempt++)); do
            [[ ! -s $port_file ]] || return 0
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.05
        done
        cat "$log" >&2
        echo "The UI fixture did not publish a port at $port_file." >&2
        return 1
    }

    if [[ $shard == all || $shard == github ]]; then
        github_fixture="$fixture_root/github"
        mkdir "$github_fixture"
        python3 "$quality_root/quality/github/github_fixture.py" "$github_fixture" --ui-control \
            > "$report/github-fixture.log" 2>&1 &
        github_pid=$!
        wait_for_fixture "$github_fixture/port" "$github_pid" "$report/github-fixture.log"
        export TEST_RUNNER_CTXFAB_GITHUB_TEST_PORT="$(< "$github_fixture/port")"
        export TEST_RUNNER_CTXFAB_GITHUB_TEST_HITS="$github_fixture/hits.log"
    fi
    if [[ $shard == all || $shard == answers ]]; then
        benchmark_fixture="$fixture_root/public-benchmark"
        mkdir "$benchmark_fixture"
        python3 "$quality_root/quality/github/github_fixture.py" "$benchmark_fixture" --public-benchmark \
            > "$report/public-benchmark-fixture.log" 2>&1 &
        benchmark_pid=$!
        wait_for_fixture "$benchmark_fixture/port" "$benchmark_pid" "$report/public-benchmark-fixture.log"
        export TEST_RUNNER_CTXFAB_BENCHMARK_GIT_PORT="$(< "$benchmark_fixture/port")"
        export TEST_RUNNER_CTXFAB_BENCHMARK_GIT_REVISION="$(< "$benchmark_fixture/revision")"
    fi
    if [[ $shard == all || $shard == answers || $shard == models ]]; then
        hub_fixture="$fixture_root/hugging-face"
        mkdir "$hub_fixture"
        python3 "$quality_root/quality/models/hugging_face_fixture.py" "$hub_fixture" --setup-q5 \
            > "$hub_fixture/server.log" 2>&1 &
        hub_pid=$!
        wait_for_fixture "$hub_fixture/port" "$hub_pid" "$hub_fixture/server.log"
        export TEST_RUNNER_CTXFAB_HUGGING_FACE_PORT="$(< "$hub_fixture/port")"
        export TEST_RUNNER_CTXFAB_HUGGING_FACE_HITS="$hub_fixture/hits.log"
    fi
    action=test
    if [[ $mode == test-built-ui ]]; then
        archive=${2:?Expected an absolute path for the XCUI archive}
        [[ $archive = /* && -f $archive ]] || { echo 'XCUI archive not found.' >&2; exit 2; }
        test_data="$fixture_root/derived"
        /usr/bin/ditto -x -k "$archive" "$test_data/Build"
        xcode=(xcodebuild -xctestrun "$test_data/Build/Products/ContextFabricator.xctestrun"
            -destination "$destination" -derivedDataPath "$test_data")
        action=test-without-building
    fi
    "${xcode[@]}" -parallel-testing-enabled NO \
        "${subjects[@]}" -resultBundlePath "$report/App.xcresult" "$action" |
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
    "${xcode[@]}" ${runtime_settings[@]+"${runtime_settings[@]}"} build
    bundle="$derived_data/Build/Products/$configuration/Context Fabricator.app"
    if [[ $configuration == Release ]]; then
        python3 -B "$quality_root/quality/release/app_notices.py" \
            "$bundle/Contents/Resources/ThirdPartyNotices" \
            --swift-checkouts "$quality_build_dir/foundation-packages/checkouts"
        /usr/bin/codesign --force --sign - --timestamp=none \
            --preserve-metadata=identifier,entitlements,flags,runtime "$bundle"
    fi
    /usr/bin/codesign --verify --deep --strict "$bundle"
fi
