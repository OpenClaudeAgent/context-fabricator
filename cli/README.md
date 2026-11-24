# CLI

`ctxfab` uses the same authenticated loopback API as the desktop. Start the app, then run `ctxfab status`, `settings`, `diagnostics`, `retrieval`, or `search QUERY`. Search defaults to BM25. Use `search QUERY --mode semantic` or `search QUERY --mode hybrid` with a configured, ready local embedding index. The `retrieval` command reports index progress and the active profile. Commands print JSON on success.

The CLI reads the active session from `CTXFAB_DATA_DIRECTORY` when set. The default is `~/Library/Application Support/ContextFabricator` on macOS and `$XDG_DATA_HOME/context-fabricator` on Linux. Linux falls back to `~/.local/share/context-fabricator` when `XDG_DATA_HOME` is unset, empty, or relative. [Qt desktop setup](../desktop/qt/README.md)

| Exit code | Meaning |
| --- | --- |
| 0 | The request succeeded. |
| 1 | Invalid discovery metadata, an unexpected response, or an output error. |
| 2 | Invalid command usage. |
| 3 | The backend session is unavailable. |
| 4 | The backend API version is incompatible. |

`ctxfab ask QUERY` submits a question to the local generation worker. Success returns JSON with `outcome` set to `answered` or `insufficientEvidence`. An answered result carries the model profile, retrieval mode, and exact quotes with their source, revision, kind and tracked ref where present. A service rejection prints the backend message and exits with status 1.

Use `ctxfab --trace ask QUERY` to print the local API's trace ID on standard error for either a response or a rejection. Standard output keeps the command's JSON format. Paste the ID into **Diagnostics → Open Trace…** to inspect the execution while it is retained. The authenticated API also serves it at `/v1/telemetry/traces/{traceId}`.

Use `--repository-id ID --branch NAME` together on `search` or `ask` to restrict the request to a tracked GitHub repository and branch. Search returns published passages. A scoped answer can also use prepared history and observed Actions evidence when available. The ID is the numeric GitHub repository ID. Use the branch name as saved in the connector, such as `main` or `release/v1`. The same filter applies to BM25, semantic and hybrid retrieval. An unindexed scope has no results. Code results include the pinned file URL and the original byte and line ranges.

Use `ctxfab models` to list provisioned generation profiles and the current selection. `ctxfab use-model PROFILE` warms and activates a profile in the running backend session. To cancel from a second terminal, submit `ctxfab ask QUERY --request-id UUID` with a fresh UUID, then run `ctxfab cancel UUID` while it is active. The cancelled answer exits with status 1 and the backend's cancellation message.

Use `ctxfab model-downloads` to list the pinned weight catalog and installation progress. Run `ctxfab install-model PROFILE` to start an installation, then repeat `ctxfab model-downloads` to check progress. If an app session ends during a transfer, run the same install command in the next session to resume installation. Verified generation weights become selectable in the running session when the runtime is available.

`ctxfab storage-plan` reads the managed build's current used, reserved and available bytes, plus the estimated peak for each runtime or model installation. Capacity is checked again when an installation starts.

On a managed build, plan the combined installation before sending any provider request. `answers` sets up generation without changing the embedding policy. A clean library then uses BM25. `hybrid` also installs the pinned embedding model and enables indexing.

```sh
ctxfab setup-plan hosted hybrid > setup-plan.json
cat setup-plan.json
operation_id=$(uuidgen)
ctxfab setup-start setup-plan.json "$operation_id"
ctxfab setup-status
```

For a local release ZIP, use `setup-plan localArchive answers` and add the ZIP path after the operation UUID in `setup-start`. An optional profile argument selects the other curated answer variant. Repeat a confirmation with the same UUID to retrieve its existing status. A confirmed hosted setup resumes under that UUID when the app starts again. It reuses verified artifacts and waits for space within the reviewed budget. If a local ZIP is still required, `ctxfab setup-archive OPERATION_UUID ZIP` supplies it and `ctxfab setup-cancel-archive OPERATION_UUID` cancels the waiting setup. Changed pins or a higher required allowance need a fresh review. The app session must be running for these commands.

The standard native macOS app and the bundled Qt distributions include their engine. `ctxfab runtime-status` reports `ready`, and the setup plan accounts for model weights. The runtime ZIP commands apply to separately configured runtime archives. [App setup](../README.md#try-it-from-source) · [Release platforms](../docs/RELEASE.md)

Use `ctxfab pause-model PROFILE` to keep staged bytes and `ctxfab resume-model PROFILE` to explicitly resume a paused or interrupted transfer. Poll `ctxfab model-downloads` to observe the transfer state. `ctxfab discard-model-download PROFILE` removes staged bytes for a stopped download.

`ctxfab model-removal-plan PROFILE` reports the local file and its logical size. Pass the plan's `identity` to `ctxfab delete-model PROFILE IDENTITY` to confirm that file. The backend rechecks the identity and rejects removal while the profile is active.

`ctxfab embedding-index` reads the saved index state. `ctxfab disable-embedding-index` pauses embedding work and preserves vectors. `ctxfab delete-embedding-index` removes vector payloads and leaves embedding work disabled until `ctxfab enable-embedding-index`. Both operations leave documents and BM25 available.

Use `ctxfab ask QUERY --repository-id ID --branch NAME --history-commit SHA` once the comparison is prepared in the app. The answer cites an exact changed line from each revision. Use `ctxfab github-actions ID` to find observed run and attempt numbers. Ask about one attempt with `ctxfab ask QUERY --repository-id ID --branch NAME --actions-run-id RUN_ID --actions-run-attempt N`. The citation carries the run URL and the verification status of its reported head commit.

Use `ctxfab github-graph 41 --branch main` to list published Rust source roots. Look up exact symbol occurrences with `ctxfab github-graph 41 --branch main --root src/lib.rs --symbol crate::target::run`. The response includes the indexed commit, source ranges, and resolved call or import targets.
