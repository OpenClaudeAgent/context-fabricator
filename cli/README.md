# CLI

`ctxfab` uses the same authenticated loopback API as the desktop. Start the macOS app, then run the embedded executable with `status`, `settings`, `diagnostics`, `retrieval`, or `search QUERY`. Search defaults to BM25. Use `search QUERY --mode semantic` or `search QUERY --mode hybrid` with a configured, ready local embedding index. The `retrieval` command reports index progress and the active profile. Commands print JSON on success.

| Exit code | Meaning |
| --- | --- |
| 0 | The request succeeded. |
| 1 | Invalid discovery metadata, an unexpected response, or an output error. |
| 2 | Invalid command usage. |
| 3 | The backend session is unavailable. |
| 4 | The backend API version is incompatible. |

`ctxfab ask QUERY` submits a question to the local generation worker. Success returns JSON with `outcome` set to `answered` or `insufficientEvidence`. An answered result carries the model profile, retrieval mode, and exact quotes with their source, revision, kind and tracked ref where present. A service rejection prints the backend message and exits with status 1.

Use `--repository-id ID --branch NAME` together on `search` or `ask` to restrict evidence to the published code for a GitHub repository and branch. The ID is the numeric GitHub repository ID. Use the branch name as saved in the connector, such as `main` or `release/v1`. The same filter applies to BM25, semantic and hybrid retrieval. An unindexed scope has no results. Code results include the pinned file URL and the original byte and line ranges.

Use `ctxfab models` to list provisioned generation profiles and the current selection. `ctxfab use-model PROFILE` warms and activates a profile in the running backend session. To cancel from a second terminal, submit `ctxfab ask QUERY --request-id UUID` with a fresh UUID, then run `ctxfab cancel UUID` while it is active. The cancelled answer exits with status 1 and the backend's cancellation message.

Use `ctxfab model-downloads` to list the pinned weight catalog and installation progress. Run `ctxfab install-model PROFILE` to start an installation in the app's running backend, then repeat `ctxfab model-downloads` to check progress. If an app session ends during a transfer, run the same install command in the next session to resume the staged bytes. Restart the app after installation to discover the new profile. Inference requires a compatible local `llama-server` executable in the app's inference directory.
