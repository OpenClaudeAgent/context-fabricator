# Sourced by the M01 commands. Keep tool installs and outputs in this checkout.
quality_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
quality_dir="$quality_root/.quality"
tools_dir="$quality_dir/tools"
export CARGO_HOME="$tools_dir/cargo"
export RUSTUP_HOME="$tools_dir/rustup"
export PATH="$tools_dir/bin:$CARGO_HOME/bin:$PATH"

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
        echo "M01 requires Apple Silicon and macOS 26." >&2
        return 1
    fi
    local installed_xcode
    installed_xcode=$(xcodebuild -version)
    if [[ "$installed_xcode" != "Xcode $xcode_version"$'\n'"Build version $xcode_build" ]]; then
        echo "Select Xcode $xcode_version ($xcode_build) with DEVELOPER_DIR." >&2
        return 1
    fi
}
