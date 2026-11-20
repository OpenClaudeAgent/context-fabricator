# macOS foundation

## Build

`make bootstrap-app` installs the pinned M01 tools and XcodeGen 2.47.0, OpenAPI Generator CLI 7.27.0, oasdiff 1.39.0, and Toxiproxy 2.13.0 under `.quality/tools/`. The release downloads are checksum-verified. `make app` generates the native project, builds from the Swift and Rust locks, and ad hoc signs the development bundle and both embedded executables. The binary under `Contents/MacOS/ctxfab` supports `status`, `settings`, `diagnostics`, and `search QUERY`.

Use `make integration` with Java, Python 3, jq, uv, and k6 on PATH. The harness pins Schemathesis 4.33.4. The reference host ran k6 2.4.0. All exercised endpoints are loopback sessions with disposable directories.

## Boundaries

- The native launcher owns the helper and its stdin lifetime pipe. Closing the window preserves the app session. Quitting closes the pipe, and a restart waits for the prior process to exit. Unexpected exits publish a typed event to the TCA store. An exclusive lock protects each private data directory. The backend writes a mode-0600 discovery record with an ephemeral bearer token and its own PID. The clients restrict discovery to loopback and disable redirects. The API rejects unauthenticated and browser-origin requests.
- SwiftUI renders backend-provided values and sends actions to a TCA store. The store owns loading, drafts, and presentation errors. Apple Swift OpenAPI Generator 1.14.4 and URLSession transport 1.4.0 generate the client from the repository's canonical schema. The Rust server uses Axum adapters and validates its live responses against that schema.
- The Rust backend owns validation, revisioned settings, and a SQLite schema with FTS5/BM25. Corpus batches update checkpoints and the searchable index in the same transaction. Collection generations invalidate stale writes after deletion. The Keychain implementation sits behind a Rust credential-store port.
- The outbound admission primitive limits handoff spacing and in-flight operations. Tests drive it with Tokio's paused clock. Provider-specific quota keys, retry policies, and durable attempt windows belong to the later connector work.
- Diagnostic events use an allowlisted code and message, bounded retention, and a 500-row cap. The telemetry fixture verifies SDK log and span correlation, metrics export, flush, and an unavailable sink. The application currently persists its diagnostic events in SQLite.

## Checks

| Command | Verification |
| --- | --- |
| `make quality` | M01 toolchain fixture checks and expected rejections. |
| `make app-analysis` | Strict SwiftLint type-aware analysis on a clean native build log. |
| `make app-unit` | Swift Testing and TCA TestStore, generated client checks against the live backend, and a 90% line coverage gate for first-party Swift package sources. |
| `make app-test` | Native XCUI tests with a separate 90% line coverage gate for the macOS app target. The Swift package has its own gate in `make app-unit`. |
| `make app-smoke` | Verify the signature, start the packaged app with isolated storage, call the live service via the embedded CLI, and verify helper shutdown. Requires `make app`. |
| `make integration` | OpenAPI validation, compatibility rejection, live conformance, authentication, k6 success and threshold rejection, and Toxiproxy outage and recovery. |
| `make foundation` | Static Swift checks, strict Rust checks and 90% backend and CLI line coverage, native unit tests, integration, build, and packaged smoke. |
| `make m02` | OpenTelemetry export, sink failure and shutdown; isolated XCTest launch-to-ready measurements; deliberate XCUI assertion failure and attachment; packaged smoke under Main Thread Checker, Address Sanitizer and Thread Sanitizer. |
| `make verify` | Run the shared toolchain, foundation, UI, runtime and packaged corpus gates in order. Reports are kept under `.quality/reports/`. |
| `make m03` | Qualify the Keychain adapter with an Apple Development-signed copy of the app. Verify credential creation across launches, denial to an ad hoc helper, rotation, removal, and correlated diagnostics. |
| `make m04` | Verify the app signature and run the same corpus fixture through the packaged backend and CLI on a fresh and an upgraded SQLite database. |
| `make m05` | Exercise the quota registry, observed HTTP attempts, durable cooldown, incremental scheduler and crash accounting. Verify the packaged helper with an isolated development fixture and the bundled CLI. |
| `make m06` | Exercise embedded OpenTelemetry correlation, age and count purge, queue saturation, slow readers, low space, and packaged shutdown drain. |
| `make m08-model` | Stage the pinned official Qwen3-Embedding-0.6B Q8_0 GGUF in `.quality/m08-assets/` and verify its SHA-256. A missing weight is fetched by the backend's counted model installer in an isolated qualification session. |
| `make m08` | Run the model download fixtures plus BM25, semantic and hybrid retrieval against judged multilingual fixtures through the packaged helper and CLI. Requires the staged model and `llama-server` build `12359` (`b1b770d90`) on `PATH` or through `CTXFAB_LLAMA_SERVER`. |
| `make m09-models` | Stage the embedding and generation profiles and link the pinned `llama-server` binary into the local asset directory. |
| `make m09` | Verify app-owned inference, grounded citations, concurrent import, worker recovery, targeted cancellation and switching between both local generation profiles. |

The standard Rust gates run fast retrieval fixtures with an isolated SQLite store and deterministic embeddings. `make m08` adds the real Qwen model and an offline Metal worker. It records per-query rankings, latency, raw vector payload and sampled worker RSS in a dated run directory under `.quality/reports/`.

## Local answer qualification

Run `make m09-models` followed by `make m09` on the supported Apple Silicon toolchain. The stage command provisions the Qwen3-Embedding-0.6B Q8_0 file and both Qwen3-4B-Instruct-2507 generation files, Q4_K_M and Q5_K_M. They take roughly 6.03 GB together. The test verifies the installed runtime build and all three hashes before launching and uses disposable corpus data. The workspace Rust gate runs the protocol double for lifecycle, citation rejection and activation rollback without downloading model weights.

For a development session with your own app data directory, use `CTXFAB_INFERENCE_DIRECTORY="$PWD/.quality/m09-assets" ".quality/foundation-xcode/Build/Products/Debug/Context Fabricator.app/Contents/MacOS/Context Fabricator"`. A standard app launch discovers the same filenames under the `inference` directory in its private application support folder. The generation and embedding processes start on demand. Report files under `.quality/reports/` retain the actual model profile, answers, citations and timing measurements.

Run `make app-test` from the logged-in macOS administrator session and keep the screen unlocked. Approve the system request to enable Automation Mode as soon as it appears. A late approval can enable the mode after XCTest has already timed out, so start a fresh run in that case. Developer Tools mode and XCTest Automation Mode have separate authorization. [Apple Xcode testing notes](https://developer.apple.com/documentation/xcode-release-notes/xcode-13-release-notes)

The native suite launches the packaged app and drives the macOS desktop. `CTXFAB_UI_SHARD=foundation make app-test`, `CTXFAB_UI_SHARD=answers make app-test` and `CTXFAB_UI_SHARD=github make app-test` limit local runs to one area. CI runs those shards on separate Macs and merges their native coverage with Xcode before enforcing the 90% application gate. Swift package and Cargo tests can run without taking focus. [Apple XCTest execution options](https://help.apple.com/xcode/mac/current/en.lproj/dev6061748b5.html)

M02 records three launch-to-backend-ready observations using XCTest's monotonic clock with code coverage disabled. Runtime qualification builds separate sanitized debug app targets and verifies their compiler instrumentation and loaded libraries, alongside the packaged launcher, backend and CLI smoke. The Rust helper is tested separately by the Rust gates. First-party C, C++ and Objective-C shims require their own diagnostics when introduced. [XCTest performance tests](https://developer.apple.com/documentation/xcode/writing-and-running-performance-tests), [Xcode runtime diagnostics](https://developer.apple.com/documentation/xcode/diagnosing-memory-thread-and-crash-issues-early)

The app starts with no provider instances and shows Overview, Answers, Connectors, Settings and Diagnostics. Run `make verify && make m03` for M03 acceptance. The Keychain check uses the sole Apple Development identity available to the local account by default. Set `CTXFAB_M03_SIGNING_IDENTITY` to select a specific identity if needed. The fixture creates its own UUID-scoped item and reports only its correlation ID and outcome. The release signing identity, signed upgrades, live connector behavior and model distribution remain qualification work.
