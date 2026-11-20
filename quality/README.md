# M01 quality gates

Run \`make bootstrap\` followed by \`make quality\` from the repository root. The reference machine is an Apple M1 Max MacBook Pro (\`MacBookPro18,2\`) with 32 GiB RAM and macOS 26.6.2 (25G83). The gates accept Apple Silicon on macOS 26 with the exact Xcode build below. Install \`jq\` if needed (\`brew install jq\`). Setup downloads upstream Rust and checksum-verified release binaries into \`.quality/tools/\`.

| Pin | Version |
| --- | --- |
| Xcode | 26.6 (17F113) |
| Swift and swift-format | 6.3.3 and 6.3.0 |
| TCA | 1.25.5 |
| SwiftLint | 0.65.1 |
| Rustup | 1.29.3 |
| Rust and Cargo | 1.100.0, Rust edition 2024 |
| rustfmt and Clippy | 1.11.0 and 0.1.100, bundled with Rust |
| cargo-deny and cargo-llvm-cov | 0.21.0 and 0.9.1 |
| jq | 1.8.2 on the reference machine |

Select the pinned Xcode with \`DEVELOPER_DIR\` if it is installed alongside another version. Xcode builds from [Package.resolved](../desktop/macos/Package.resolved) with package updates disabled. Cargo uses [Cargo.lock](m01-rust/Cargo.lock) with \`--locked\`. Review lockfile changes along with changes to dependency declarations. Xcode's command-line macro approval is scoped to the fixture build so the locked TCA macros can run on a fresh checkout. Review macro dependency changes as executable build inputs.

## Standards enforced

- **Swift.** The official formatter owns source layout with two-space indentation and a 100-column target. Swift 6 enforces isolation and sendability, and both first-party targets treat compiler warnings as errors. SwiftLint's strict rules cover unsafe casts, unwraps and forced tries, unhandled throwing tasks, concurrency annotations, capture and delegate safety, maintainability and avoidable collection passes. The type-aware rules check unused imports, unused declarations and mutable closure captures. Naming clarity and API design also require review. [Swift API Design Guidelines](https://www.swift.org/documentation/api-design-guidelines/), [Swift 6 concurrency migration guide](https://www.swift.org/migration/documentation/swift-6-concurrency-migration-guide/enabledataracesafety/), [Google Swift Style Guide](https://google.github.io/swift/), [SwiftLint rule directory](https://realm.github.io/SwiftLint/rule-directory.html)
- **Rust.** rustfmt owns layout. Workspace members inherit Clippy's default and pedantic groups and explicit restrictions on unchecked unwraps, panics, debugging and unfinished macros, and undocumented unsafe blocks. rustc denies implicit unsafe operations inside unsafe functions. CI checks all workspace targets and features with warnings denied. cargo-deny checks RustSec advisories, approved licenses and registries, and duplicate versions against the locked graph. The initial Rust license allowlist is Apache-2.0 and MIT. Review any addition to the policy with the dependency change. Use the Rust API Guidelines for API review. [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/), [Clippy lint groups](https://doc.rust-lang.org/stable/clippy/lints.html), [Rust 2024 unsafe operations](https://doc.rust-lang.org/edition-guide/rust-2024/unsafe-op-in-unsafe-fn.html), [cargo-deny](https://embarkstudios.github.io/cargo-deny/)
- **Coverage.** Enforce a separate 90% minimum line coverage for each first-party fixture. Xcode and \`xccov\` measure Swift files under \`Sources\`. \`cargo-llvm-cov\` measures Rust workspace source with test files excluded. Both reject an uncovered branch while the tests still pass. Review coverage changes alongside behavior assertions. [Xcode test results](https://developer.apple.com/documentation/xcode/running-tests-and-interpreting-results), [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov)

## Reproduction and reports

\`make format\` applies the native formatters to the Swift package and Rust workspace. \`make quality\` starts with fresh fixture source copies and build directories. It runs the checks on clean code, then introduces faults using [the fixtures](fixtures). Every expected failure must exit nonzero and match its intended diagnostic or report. An unexpected outcome exits nonzero. The runner currently stages the M01 Swift package and the isolated Rust fixture workspace under `quality/m01-rust`. Update it when adding workspace members or new first-party test targets.

Each run creates a \`.quality/reports/run.*\` directory with versions, locks, compiler and analyzer logs, test summaries and Xcode result bundles, native coverage reports, and an index of expected rejections. Type-aware SwiftLint uses a clean, non-instrumented build so the macro executables can run inside SourceKit without coverage-profile writes. The raw build log is retained and the analyzer input unescapes Xcode's macro path separator. The coverage build records a separate Xcode test result.

The [GitHub Actions workflow](../.github/workflows/quality.yml) runs the same commands on Apple Silicon and uploads reports even on failure. The commands can be run locally without a Git remote.
