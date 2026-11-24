#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

app=${1:-"$quality_build_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"}
diagnostic=${2:-}
macos="$app/Contents/MacOS"
mkdir -p "$quality_dir/reports"
quality_begin_report reports bundle
work=$(quality_workspace bundle)
app_pid=
cleanup() {
    if [[ -n $app_pid ]]; then kill "$app_pid" 2>/dev/null || true; wait "$app_pid" 2>/dev/null || true; fi
    rm -rf -- "$work"
}
quality_on_exit cleanup

codesign --verify --deep --strict "$app"
launch_env=("LLVM_PROFILE_FILE=$work/app-%p.profraw" "CTXFAB_TEST_DATA_DIRECTORY=$work/data")
case "$diagnostic" in
    "") ;;
    main-thread)
        library="$(xcode-select -p)/usr/lib/libMainThreadChecker.dylib"
        test -f "$library"
        launch_env+=("DYLD_INSERT_LIBRARIES=$library" "DYLD_PRINT_LIBRARIES=1")
        runtime=libMainThreadChecker.dylib
        ;;
    address)
        launch_env+=("DYLD_PRINT_LIBRARIES=1" "ASAN_OPTIONS=halt_on_error=1")
        runtime=libclang_rt.asan_osx_dynamic.dylib
        ;;
    thread)
        launch_env+=("DYLD_PRINT_LIBRARIES=1" "TSAN_OPTIONS=halt_on_error=1")
        runtime=libclang_rt.tsan_osx_dynamic.dylib
        ;;
    *)
        echo "Unknown runtime diagnostic: $diagnostic" >&2
        exit 2
        ;;
esac
env "${launch_env[@]}" "$macos/Context Fabricator" > "$reports/app.log" 2>&1 &
app_pid=$!
endpoint="$work/data/endpoint.json"
for (( attempt=0; attempt < 100; attempt++ )); do
    [[ -s $endpoint ]] && break
    kill -0 "$app_pid" 2>/dev/null || break
    sleep 0.1
done
if [[ ! -s $endpoint ]]; then
    cat "$reports/app.log" >&2
    echo "The packaged launcher did not start the backend." >&2
    exit 1
fi
helper_pid=$(jq -er .processId "$endpoint")
CTXFAB_DATA_DIRECTORY="$work/data" "$macos/ctxfab" status |
    tee "$reports/status.json" |
    jq -e '.status == "ready" and .apiVersion == 1 and .connectedSources == 0' > /dev/null
CTXFAB_DATA_DIRECTORY="$work/data" "$macos/ctxfab" diagnostics |
    tee "$reports/diagnostics.json" | jq -e 'any(.[]; .code == "backend_started")' > /dev/null
kill "$app_pid"
stop_status=0
wait "$app_pid" 2>/dev/null || stop_status=$?
app_pid=
[[ $stop_status == 0 || $stop_status == 143 ]]
for (( attempt=0; attempt < 50; attempt++ )); do
    [[ ! -e $endpoint ]] && ! kill -0 "$helper_pid" 2>/dev/null && break
    sleep 0.1
done
[[ ! -e $endpoint ]] && ! kill -0 "$helper_pid" 2>/dev/null || {
    echo "The bundled backend survived launcher termination." >&2
    exit 1
}
if [[ -n "$diagnostic" ]]; then
    if ! grep -F "$runtime" "$reports/app.log" > "$reports/runtime-library.log" ||
        ! grep -Fq "dyld[" "$reports/runtime-library.log"; then
        echo "The app did not report loading $runtime. See $reports/app.log." >&2
        exit 1
    fi
    if grep -Ei 'Main Thread Checker:|ERROR: AddressSanitizer|WARNING: ThreadSanitizer|runtime error:|UndefinedBehaviorSanitizer:' "$reports/app.log" > "$reports/runtime-findings.log"; then
        cat "$reports/runtime-findings.log" >&2
        exit 1
    fi
    echo "Loaded $runtime in the packaged app. No runtime findings. Reports in $reports"
fi
echo "Verified bundled backend and CLI, with shutdown when the launcher exits. Reports in $reports"
