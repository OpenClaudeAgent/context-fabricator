# Quality checks

Use `make bootstrap-app` to install the pinned tools on Apple Silicon with the project’s Xcode version. The commands run from the repository root.

| What to check | Command | Where |
| --- | --- | --- |
| Rust, CLI and Swift package tests without opening the app | `make test-headless` | [Makefile](../Makefile) |
| Format, strict analysis, coverage and OpenAPI checks | `make check-product` | [Checks](checks/) · [Contracts](contracts/) |
| Tool versions and fixtures that demonstrate rejection | `make qualify-tools` | [Toolchain](toolchain/) |
| Build the app and exercise the packaged helper | `make qualify-debug-bundle` | [macOS](macos/) · [Collection](collection/) · [Observability](observability/) |
| Rendered windows, keyboard navigation and UI coverage | `make test-native-ui` | [Native tests](../desktop/macos/UITests/) |
| Qt UI and window scenarios | `ctest --preset quality` from `desktop/qt` · `run_window_scenarios.py` | [Qt checks](../desktop/qt/README.md#qualité) · [Qt CI](../.github/workflows/qt.yml) |
| GitHub collection and grounded answers | `make qualify-github` · `make qualify-github-answers` | [GitHub](github/) |
| Model retrieval and local generation | `make qualify-retrieval` · `make qualify-answers` | [Models](models/) · [Retrieval](retrieval/) · [Answers](answers/) |
| Release ZIP, signatures, notices and upgrade | `make qualify-release` | [Release](release/) |

Coverage gates require 90% of first-party lines in each Rust workspace, the Swift package and the app's native suite. The native suite drives a macOS desktop session. Reports live under `.quality/reports/` with names such as `telemetry.*` and `release-package.*`.

[Release preparation](../docs/RELEASE.md) · [CI workflow](../.github/workflows/quality.yml) · [RAG benchmark methodology](github/uv_benchmark/README.md#scientific-basis) · [Java benchmark questions](github/java_benchmark/QUESTIONS.md) · [Performance checks](performance/) · [Storage checks](storage/)

Pull requests and pushes to `main` run the applicable [Rust](../.github/workflows/ci-rust.yml), [Swift](../.github/workflows/ci-swift.yml) and [Qt](../.github/workflows/ci-qt.yml) checks in parallel. The target is ten minutes from runner allocation. All three use real tests, and the Swift and Qt suites exercise a Rust backend over loopback. Qt also replays the window scenarios offscreen on Linux.

The [Release candidate workflow](../.github/workflows/release.yml) runs both qualifications on the same commit. macOS runs the product, diagnostic and XCUI suites, merges the native coverage, and verifies the Release ZIP. Qt runs static analysis, coverage and sanitizers on Linux and macOS, and verifies the Debian install on X11 and Wayland. Each qualification can also be run individually for troubleshooting.

To include the two XCUI scenarios that download pinned public repositories, run `CTXFAB_UI_SHARD=answers CTXFAB_XCTEST_LIVE_BENCHMARK=1 make test-native-ui` in a macOS desktop session.

Local commands write active reports and disposable builds in temporary workspaces. At exit, they retain up to two finished reports per check within the **2 GB total budget for `.quality/`**. Legacy files count against that budget. CI keeps its reports for upload.

Local Xcode builds and `make test-headless` reclaim a Cargo workspace cache when it exceeds **12 GB**. Rust test builds use line tables and omit incremental snapshots. Coverage uses a temporary Cargo target that is removed after the command.

Pinned tools, staged model weights and reusable native builds use the system cache at `~/Library/Caches/ContextFabricator` on macOS and `${XDG_CACHE_HOME:-~/.cache}/ContextFabricator` on Linux. Run `python3 -B quality/toolchain/cache-directory.py --build` for the native build directory of this checkout. You can set `CTXFAB_CACHE_DIR` to a cache directory outside `.quality`. Set `CTXFAB_RELEASE_PACKAGE_DIR` to a new directory outside `.quality` when a release package must be kept independently of its qualification report.
