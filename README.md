# Context Fabricator

**Find the evidence you need across the engineering tools you already use.**

Context Fabricator is an open-source desktop app for building a local, searchable index of engineering data. Connect a GitHub repository, see what has been collected, and ask questions from the app or the `ctxfab` CLI. Citations take you back to the original evidence.

> **Preview · AI-generated implementation.** AI coding agents generate the code, tests and documentation. A human directs the product, makes architecture decisions and tests the app. The native SwiftUI client targets Apple Silicon. The Qt Quick client targets Ubuntu 24.04 amd64 and macOS arm64. [Release preparation](docs/RELEASE.md)

| macOS · SwiftUI | Qt Quick |
| --- | --- |
| [![macOS Answers showing a local response and its source excerpt](docs/screenshots/answers.png)](docs/SCREENSHOTS.md) | [![Qt Answers showing a local response and its source excerpt](docs/screenshots/qt/answers.png)](docs/SCREENSHOTS-QT.md) |
| [macOS gallery](docs/SCREENSHOTS.md) | [Qt gallery](docs/SCREENSHOTS-QT.md) |

## What works today

GitHub is the first connector in the preview. Code citations link to the exact commit, file, and lines.

- **Read-only GitHub indexing.** Choose a repository, search its branches, then enable branch files, commit history, GitHub Actions or the Rust code graph. The connector fetches branch files through Git Smart HTTP and shows request pacing and storage usage. Credentials are stored in Keychain on macOS and Secret Service on Linux.
- **Local search and answers.** SQLite/FTS5 provides BM25 search. Optional Qwen3 embeddings enable semantic and hybrid search. Qwen3 4B generates answers locally and the backend checks the quoted evidence.
- **Inspect Pipeline and replay benchmarks.** See the sources searched, retrieved passages, ranking, model input, draft and citation verification. Import test data or activate pinned repositories and workflow attempts, then run a question or a suite. [Pipeline architecture](ARCHITECTURE.md#pipeline-inspection-and-benchmark-replay)
- **Built-in diagnostics.** Follow a question from search candidates to the answer and verified citations. When the backend withholds an answer, the trace can show the sources searched and the reason. Diagnostics also shows request activity, filtered logs, metrics and storage.
- **Terminal access.** The bundled `ctxfab` CLI uses the same local backend and can scope search and answers to a repository and branch.
- **Eight interface languages.** Choose English, French, German, Spanish, Italian, Portuguese, Japanese or Simplified Chinese in **App Language** in Settings. The system language is the default. Answer language and history are grouped in **Settings → Answers**.

History answers verify both sides of a prepared comparison. Actions answers cite the selected run and attempt. The Rust graph resolves a limited set of imports and qualified calls. Datadog, Sonar and other connectors are planned.

## Try it from source

### Native macOS app

Use Apple Silicon, macOS 26, Xcode 26.6 (17F113), Python 3.12 or later, CMake, Ninja, and `jq`. Run from the repository root. The build includes the pinned inference engine.

```sh
make bootstrap-app
make package-debug
build="$(python3 -B quality/toolchain/cache-directory.py --build)"
open "$build/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
```

To build from Xcode after `make bootstrap-app`, run `make xcode-project` and open `desktop/macos/ContextFabricator.xcodeproj`. Select the **ContextFabricator** scheme and **My Mac**, then choose **Product → Run**. Run `make xcode-project` again after adding, moving, or removing app or UI test files so Xcode picks up the new layout.

For GitHub testing across Debug rebuilds, find your Apple Development identity with `security find-identity -v -p codesigning`. Put its fingerprint in `desktop/macos/LocalSigning.xcconfig` so Xcode signs the Rust backend with the same identity on each build. If the token was saved by an ad hoc build, replace it once from **Connectors → GitHub** after rebuilding.

```xcconfig
CTXFAB_BACKEND_SIGNING_IDENTITY = YOUR_APPLE_DEVELOPMENT_CERTIFICATE_FINGERPRINT
```

1. Open **Connectors → GitHub** and use the browser setup to create a fine-grained token with **Contents: Read** for the repositories you select. Add **Actions: Read** to collect workflow runs. Paste the token into the app, add a repository and choose its branch. Open Repository Settings and use **Refresh Now** to queue a check.
2. Open **Answers → Choose a Model…**. Select a built-in answer model or a compatible profile added from Hugging Face. If you enable semantic search, choose Qwen3 Embedding 0.6B Q8_0 or F16. Review the download and disk space estimate before starting. **Settings → Manage Models…** also lets you download individual models. [Model lifecycle](ARCHITECTURE.md#local-inference)

3. Ask a question in **Answers** with **Command-Return**, or try the bundled CLI while the app is running.

```sh
build="$(python3 -B quality/toolchain/cache-directory.py --build)"
CTXFAB="$build/foundation-xcode/Build/Products/Debug/Context Fabricator.app/Contents/MacOS/ctxfab"
"$CTXFAB" status
"$CTXFAB" search "timeout"
```

See [CLI usage](cli/README.md) for repository filters, model controls and answer commands.

### Qt desktop

Build the Qt client with the `dev` CMake preset, then run it with the local Rust backend. The Qt guide covers the application bundle on macOS and the Ubuntu package. [Build and run Qt](desktop/qt/README.md) · [Platform scope](docs/qt-port.md)

## Replay a benchmark

Open **Answers → Inspect Pipeline → Sources** and download or activate a pinned corpus. In **Benchmarks**, select that question set and choose **Run Question** or **Run All**. **Inspect Result** opens the evidence for a completed question. Use **BM25** for a keyword run or **Hybrid** to prepare and use the local embedding index for that corpus. [macOS walkthrough](docs/SCREENSHOTS.md#pipeline-inspection) · [Qt walkthrough](docs/SCREENSHOTS-QT.md#pipeline-inspection)

## Project layout

See [Architecture](ARCHITECTURE.md) for component boundaries, data flow and the model lifecycle.

| Path | Responsibility |
| --- | --- |
| [desktop/macos](desktop/macos) | SwiftUI interface, TCA presentation state and macOS launcher. |
| [desktop/qt](desktop/qt) | Qt Quick and C++ desktop client for Linux and macOS. [Platform scope](docs/qt-port.md) |
| [backend](backend) | Rust service for collection, SQLite storage, retrieval, inference adapters and observability. |
| [cli](cli) | Rust command-line client for the app's local service. |
| [contracts](contracts) | OpenAPI contract shared by the backend and its clients. |
| [quality](quality) | Build tooling, test fixtures and quality gates. Finished local reports go under `.quality/reports/`. |

Run `make test-headless` for Rust, CLI and Swift package tests, including loopback integration tests across the generated client and backend. `make check-product` adds strict static analysis, workspace coverage and live OpenAPI contract checks. The [quality guide](quality/README.md) covers native XCTest, Qt window scenarios, benchmark fixtures and packaging.

## License

Context Fabricator is available under the [MIT License](LICENSE).
