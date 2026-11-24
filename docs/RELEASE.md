# Release preparation

Context Fabricator is in preview. The native macOS project declares version `0.1.0`. Use the same source revision for the release notes, package qualification and screenshots. The guides below describe the candidate scope.

## Candidate scope

| Client | Distribution | Qualification |
| --- | --- | --- |
| Native macOS | Apple Silicon app with the Rust backend, CLI and bundled llama.cpp runtime | Build the Release ZIP with `make qualify-release`. For public distribution, verify the Developer ID identity, notarization and first launch on a clean Mac. [macOS workflow](../.github/workflows/quality.yml) · [Distribution verifier](../quality/release/check-distribution.sh) |
| Qt on Ubuntu | Ubuntu 24.04 amd64 `.deb` with X11, Wayland and a bundled llama.cpp CPU runtime | Verify installation, first launch, CLI discovery and profile retention on upgrade. Use the workflow's `native_inference` option for the cited-answer check with the catalog weights. [Qt workflow](../.github/workflows/qt.yml) · [Package guide](../desktop/qt/README.md#paquet-ubuntu-2404-amd64) |
| Qt on macOS | Local Apple Silicon preview bundle | Verify Cocoa startup, backend and runtime discovery. Public distribution requires signing and notarization for this bundle and a clean-machine run. [Local bundle guide](../desktop/qt/README.md#bundle-macos-local) |

## User-facing changes to highlight

- Local Answers with source excerpts, exact GitHub citations and optional answer history.
- GitHub branch files, commit history, observed Actions attempts and a scoped Rust code graph.
- Inspect Pipeline and replay pinned questions against test corpora or selected workflow attempts.
- Guided generation and embedding setup, optional semantic search, and local diagnostics.

[README](../README.md) · [Architecture](../ARCHITECTURE.md) · [macOS gallery](SCREENSHOTS.md) · [Qt gallery](SCREENSHOTS-QT.md)

## Evidence for publication

Run [Release candidate](../.github/workflows/release.yml) to qualify both packages and produce a single artifact with their checksums. Enable `create_tag` to create the version tag on the qualified commit and prepare a GitHub prerelease draft. Pushing an existing version tag also starts the workflow.

The macOS ZIP in the draft uses an ad hoc signature. Replace it with a Developer ID-signed and notarized ZIP and update the checksums before publishing. The Release candidate workflow leaves the draft unpublished.

Record the source revision, package checksum, platform, installation and upgrade results, and one verified answer from each package in scope. Keep the fixture UI results and real-weight inference results identified in the release report. Include the packaged third-party notices and model download requirements with the release assets. Use a fresh pair of native and Qt screenshots when qualifying the final candidate. [Quality checks](../quality/README.md)
