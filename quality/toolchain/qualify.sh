#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/environment.sh"

mkdir -p "$quality_dir/reports"
quality_begin_report reports toolchain
work=$(quality_workspace toolchain-work)
quality_on_exit 'rm -rf -- "$work"'
echo "Reports: $reports"
fixtures="$quality_root/quality/toolchain/fixtures"

run() {
    local lane=$1
    shift
    printf '\n%s\n' "== $lane =="
    "$@" 2>&1 | tee "$reports/$lane.log"
}

record() {
    local lane=$1
    shift
    "$@" > "$reports/$lane.log" 2>&1 || {
        tail -n 80 "$reports/$lane.log" >&2
        return 1
    }
}

require_text() {
    local expression=$1 log=$2
    if ! grep -Eq -- "$expression" "$log"; then
        echo "Missing expected evidence in $log: $expression" >&2
        return 1
    fi
}

reject() {
    local lane=$1 diagnostic=$2 status=0
    shift 2
    printf '\n%s\n' "== $lane (expected rejection) =="
    "$@" > "$reports/$lane.log" 2>&1 || status=$?
    if [[ $status -eq 0 ]] || ! require_text "$diagnostic" "$reports/$lane.log"; then
        tail -n 50 "$reports/$lane.log" >&2
        echo "$lane did not reject the intended fixture (exit $status)." >&2
        return 1
    fi
    printf '%s exited %d\n' "$lane" "$status" | tee -a "$reports/rejections.txt"
    grep -E -- "$diagnostic" "$reports/$lane.log"
}

versions() {
    require_mac || return
    command -v jq > /dev/null || { echo "Install jq before running the gates." >&2; return 1; }
    local rust_version
    rust_version=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$quality_root/rust-toolchain.toml")
    sw_vers
    sysctl -n hw.model
    xcodebuild -version
    xcrun swift --version
    xcrun swift-format --version
    rustup --version
    rustc --version
    cargo --version
    cargo fmt --version
    cargo clippy --version
    swiftlint version
    cargo deny --version
    cargo llvm-cov --version
    jq --version
    if [[ $(xcrun swift --version) != *"Swift version $swift_version "* ||
          $(xcrun swift-format --version) != "$swift_format_version" ||
          $(rustc --version) != "rustc $rust_version "* ||
          $(swiftlint version) != "$swiftlint_version" ||
          $(cargo deny --version) != "cargo-deny $cargo_deny_version" ||
          $(cargo llvm-cov --version) != "cargo-llvm-cov $cargo_llvm_cov_version" ]]; then
        echo "Toolchain version mismatch. Select the pinned Xcode and run make bootstrap." >&2
        return 1
    fi
}

xcode() {
    local derived=$1
    shift
    xcodebuild -scheme M01QualityFixture-Package -destination 'platform=macOS,arch=arm64' \
        -configuration Debug -derivedDataPath "$work/$derived" \
        -clonedSourcePackagesDirPath "$work/SourcePackages" \
        -onlyUsePackageVersionsFromResolvedFile -skipPackageUpdates \
        -skipMacroValidation "$@"
}

swift_tests() {
    local lane=$1
    shift
    xcode coverage -enableCodeCoverage YES -resultBundlePath "$reports/$lane.xcresult" "$@" test
}

swift_test_summary() {
    local lane=$1
    xcrun xcresulttool get test-results summary --path "$reports/$lane.xcresult" > "$reports/$lane.json"
}

swift_analyze() {
    local lane=$1
    # SwiftLint 0.65.1 does not unescape Xcode's \# separator in macro executable arguments.
    sed 's/\\#/#/g' "$reports/$lane-build.log" > "$work/$lane-compiler.log"
    swiftlint analyze --strict --config .swiftlint.yml --compiler-log-path "$work/$lane-compiler.log" Sources Tests
}

swift_coverage() {
    local lane=$1
    local result="$reports/$lane.xcresult"
    xcrun xccov view --report "$result" > "$reports/$lane-coverage.txt" || return
    xcrun xccov view --report --json "$result" > "$reports/$lane-coverage.json" || return
    jq -er --arg prefix "$PWD/Sources/" --argjson minimum "$minimum_line_coverage" '
        [.targets[].files[] | select(.path | startswith($prefix))] | unique_by(.path)
        | (map(.coveredLines) | add // 0) as $covered
        | (map(.executableLines) | add // 0) as $total
        | select($total > 0)
        | "\($covered)/\($total) first-party Swift lines, minimum \($minimum)%",
          ($covered * 100 >= $minimum * $total)
    ' "$reports/$lane-coverage.json"
}

rust_coverage() {
    local lane=$1
    cargo llvm-cov --locked --workspace --ignore-filename-regex '(^|/)tests/' \
        --fail-under-lines "$minimum_line_coverage" --json --output-path "$reports/$lane.json"
}

run versions versions
mkdir "$work/swift" "$work/rust" "$work/snippets"
cp "$quality_root/desktop/macos/"{.swift-format,.swiftlint.yml,Package.swift,Package.resolved} "$work/swift/"
cp -R "$quality_root/desktop/macos/"{Sources,Tests} "$work/swift/"
cp "$quality_root/quality/toolchain/rust-fixture/"{Cargo.toml,Cargo.lock,deny.toml} "$work/rust/"
cp "$quality_root/rust-toolchain.toml" "$work/rust/"
cp -R "$quality_root/quality/toolchain/rust-fixture/quality-fixture" "$work/rust/"
cp "$work/swift/Package.resolved" "$work/rust/Cargo.lock" "$reports/"

cd "$work/swift"
run swift-format xcrun swift-format lint --strict --configuration .swift-format --recursive Package.swift Sources Tests
run swift-lint swiftlint lint --strict --no-cache --config .swiftlint.yml Package.swift Sources Tests
if grep -Eq '^warning:' "$reports/swift-lint.log"; then
    echo "SwiftLint reported a configuration warning." >&2
    exit 1
fi
record swift-analyze-build xcode analysis -enableCodeCoverage NO build-for-testing
for module in QualityFixture QualityFixtureTests; do
    grep -E "swiftc -module-name $module " "$reports/swift-analyze-build.log" |
        grep -- '-swift-version 6 ' |
        grep -- '-warnings-as-errors ' > "$reports/swift-compiler-$module.log"
done
run swift-analyze swift_analyze swift-analyze
swift_files=$(find Sources Tests -type f -name '*.swift' | wc -l | tr -d ' ')
require_text "Found 0 violations, 0 serious in $swift_files files\\.$" "$reports/swift-analyze.log"
if grep -Ei '(^|: )(error:|warning:)|LLVM Profile Error' "$reports/swift-analyze.log"; then
    echo "Analyzer reported a compiler or configuration diagnostic." >&2
    exit 1
fi

echo "== Swift tests and coverage =="
record swift-tests swift_tests swift-tests
swift_test_summary swift-tests
jq -e '.passedTests == 2 and .failedTests == 0' "$reports/swift-tests.json" > /dev/null
for test_name in unitFixture scenarioFixture; do
    require_text "Test $test_name\\(\\) passed" "$reports/swift-tests.log"
done
run swift-coverage swift_coverage swift-tests

cd "$work/rust"
run rust-format cargo fmt --all --check
run rust-clippy cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
run rust-tests cargo test --locked --workspace
require_text 'test tests::unit_fixture \.\.\. ok' "$reports/rust-tests.log"
run rust-dependencies cargo deny --locked check
run rust-coverage rust_coverage rust-coverage-data
jq -e '.data[0].totals.lines.count > 0' "$reports/rust-coverage-data.json" > /dev/null
run rust-coverage-summary cargo llvm-cov report --ignore-filename-regex '(^|/)tests/'
require_text 'quality-fixture/src/lib\.rs' "$reports/rust-coverage-summary.log"

# Each red check matches a diagnostic from the intended fixture. All mutations stay in the staged copy.
cd "$work/swift"
cp "$fixtures/swift/"{Formatting,Lint,Concurrency}.swift "$work/snippets/"
reject swift-format-red 'Formatting\.swift:.*error: \[Spacing\]' \
    xcrun swift-format lint --strict --configuration .swift-format "$work/snippets/Formatting.swift"
reject swift-lint-red 'Lint.swift:.*\(force_cast\)' \
    swiftlint lint --strict --no-cache --config .swiftlint.yml "$work/snippets/Lint.swift"
require_text 'Lint.swift:.*\(force_unwrapping\)' "$reports/swift-lint-red.log"
require_text 'Lint.swift:.*\(unhandled_throwing_task\)' "$reports/swift-lint-red.log"
reject swift-compiler-red 'Concurrency.swift:.*error:.*main actor-isolated' \
    xcrun swiftc -typecheck -swift-version 6 -warnings-as-errors "$work/snippets/Concurrency.swift"

cp "$fixtures/swift/Analyzer.swift" Sources/QualityFixture/
record swift-analyze-red-build xcode analysis -enableCodeCoverage NO build-for-testing
reject swift-analyze-red 'Analyzer.swift:.*\(unused_import\)' swift_analyze swift-analyze-red
require_text 'Analyzer.swift:.*\(unused_declaration\)' "$reports/swift-analyze-red.log"
rm Sources/QualityFixture/Analyzer.swift

patch -sp1 -i "$fixtures/swift/unit.patch"
reject swift-unit-red 'UnitTests.swift:.*Expectation failed:' swift_tests swift-unit-red
swift_test_summary swift-unit-red
jq -e '.failedTests == 1 and .passedTests == 1' "$reports/swift-unit-red.json" > /dev/null
patch -Rsp1 -i "$fixtures/swift/unit.patch"

patch -sp1 -i "$fixtures/swift/scenario.patch"
reject swift-scenario-red 'A state change does not match expectation' swift_tests swift-scenario-red
require_text 'Test scenarioFixture\(\) recorded an issue at ScenarioTests.swift:' "$reports/swift-scenario-red.log"
swift_test_summary swift-scenario-red
jq -e '.failedTests == 1 and .passedTests == 1' "$reports/swift-scenario-red.json" > /dev/null
patch -Rsp1 -i "$fixtures/swift/scenario.patch"

patch -sp1 -i "$fixtures/swift/coverage.patch"
record swift-coverage-red-test swift_tests swift-coverage-red-test
swift_test_summary swift-coverage-red-test
jq -e '.passedTests == 2 and .failedTests == 0' "$reports/swift-coverage-red-test.json" > /dev/null
reject swift-coverage-red '^[0-9]+/[0-9]+ first-party Swift lines' swift_coverage swift-coverage-red-test
require_text '^false$' "$reports/swift-coverage-red.log"

cd "$work/rust"
cp "$fixtures/rust/format.rs" quality-fixture/src/lib.rs
reject rust-format-red 'Diff in .*quality-fixture/src/lib\.rs:' cargo fmt --all --check
cp "$fixtures/rust/clippy.rs" quality-fixture/src/lib.rs
reject rust-clippy-red 'clippy::unwrap-used' cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
require_text 'clippy::needless_bool' "$reports/rust-clippy-red.log"
cp "$quality_root/quality/toolchain/rust-fixture/quality-fixture/src/lib.rs" quality-fixture/src/lib.rs

patch -sp1 -i "$fixtures/rust/unit.patch"
reject rust-unit-red 'test tests::unit_fixture \.\.\. FAILED' cargo test --locked --workspace
require_text 'assertion .* failed' "$reports/rust-unit-red.log"
patch -Rsp1 -i "$fixtures/rust/unit.patch"

reject rust-licenses-red 'error\[rejected\]: failed to satisfy license requirements' \
    cargo deny --locked --config "$fixtures/rust/deny.toml" check licenses
require_text 'itoa-1\.0\.15|itoa v1\.0\.15' "$reports/rust-licenses-red.log"

cargo llvm-cov clean --workspace
patch -sp1 -i "$fixtures/rust/coverage.patch"
reject rust-coverage-red 'test tests::unit_fixture \.\.\. ok' rust_coverage rust-coverage-red-data
jq -e --argjson minimum "$minimum_line_coverage" '
    .data[0].totals.lines | .count > 0 and .percent < $minimum
' "$reports/rust-coverage-red-data.json" > /dev/null
run rust-coverage-red-summary cargo llvm-cov report --ignore-filename-regex '(^|/)tests/'

echo "Toolchain qualification passed. Reports in $reports"
