# Shared by the quality and app build commands.
quality_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
quality_dir="$quality_root/.quality"
if [[ -n ${CI:-} ]]; then
    quality_cache_dir="$quality_dir"
    quality_build_dir="$quality_dir"
else
    cache_locator="$quality_root/quality/toolchain/cache-directory.py"
    quality_cache_dir=$(python3 -B "$cache_locator" --checkout "$quality_root") || return
    quality_build_dir=$(python3 -B "$cache_locator" --checkout "$quality_root" --build) || return
fi
tools_dir="$quality_cache_dir/tools"

quality_use_managed_rust() {
    export CARGO_HOME="$tools_dir/cargo"
    export RUSTUP_HOME="$tools_dir/rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
}

export PATH="$tools_dir/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
if [[ -n ${CI:-} ]]; then
    quality_use_managed_rust
else
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    if ! command -v cargo >/dev/null 2>&1 && [[ -x "$tools_dir/cargo/bin/cargo" ]]; then
        quality_use_managed_rust
    fi
fi

xcode_version=26.6
xcode_build=17F113
swift_version=6.3.3
swift_format_version=6.3.0
rustup_version=1.29.3
swiftlint_version=0.65.1
cargo_deny_version=0.21.0
cargo_llvm_cov_version=0.9.1
minimum_line_coverage=90

require_mac() {
    if [[ $(uname -s) != Darwin || $(uname -m) != arm64 || $(sw_vers -productVersion) != 26.* ]]; then
        echo "The quality tools require Apple Silicon and macOS 26." >&2
        return 1
    fi
    local installed_xcode
    installed_xcode=$(xcodebuild -version)
    if [[ "$installed_xcode" != "Xcode $xcode_version"$'\n'"Build version $xcode_build" ]]; then
        echo "Select Xcode $xcode_version ($xcode_build) with DEVELOPER_DIR." >&2
        return 1
    fi
}

quality_begin_report() {
    quality_report_dir=$(python3 -B "$quality_root/quality/toolchain/report-directory.py" "$quality_dir/reports" "$2" "$$") || return
    printf -v "$1" '%s' "$quality_report_dir"
    trap quality_finish EXIT
}

quality_on_exit() {
    quality_cleanup_hook=$1
    trap quality_finish EXIT
}

quality_finish() {
    local status=$?
    trap - EXIT
    if [[ -n ${quality_cleanup_hook:-} ]]; then
        eval "$quality_cleanup_hook" || {
            if [[ $status == 0 ]]; then status=1; fi
        }
    fi
    if [[ -n ${quality_report_dir:-} ]]; then
        python3 -B "$quality_root/quality/toolchain/report-directory.py" \
            "$quality_report_dir" finish "$$" --destination "$quality_dir/reports" || {
            if [[ $status == 0 ]]; then status=1; fi
        }
    fi
    exit "$status"
}

quality_workspace() {
    local parent=${TMPDIR:-/tmp}
    mktemp -d "${parent%/}/ctxfab-$1.XXXXXX"
}

quality_bound_cargo_cache() {
    local workspace=$1 target usage
    [[ -z ${CI:-} ]] || return 0
    target="$workspace/target"
    [[ -d "$target" ]] || return 0
    usage=$(du -skx "$target") || return
    (( ${usage%%[[:space:]]*} <= 11718750 )) && return 0
    if [[ -L "$target" || ! -f "$target/CACHEDIR.TAG" ]]; then
        echo "error: Cargo cache exceeds 12 GB and cannot be cleaned safely: $target" >&2
        return 1
    fi
    echo "Cargo cache exceeds 12 GB. Clearing generated output in $target."
    cargo clean --locked --manifest-path "$workspace/Cargo.toml" --target-dir "$target"
}

quality_cargo_test() {
    local workspace=$1 status=0
    shift
    quality_bound_cargo_cache "$workspace" || return
    cargo test --manifest-path "$workspace/Cargo.toml" "$@" || status=$?
    quality_bound_cargo_cache "$workspace" || {
        if [[ $status == 0 ]]; then status=1; fi
    }
    return "$status"
}
