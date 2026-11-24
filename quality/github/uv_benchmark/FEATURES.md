# GitHub feature benchmark

This benchmark asks 12 distinct questions about branch publication, first-parent history, the Rust graph, Actions provenance, and agreement across those features. The questions, expected observations, and their oracles are fixed in [feature-gold.json](feature-gold.json). The English and French versions of a question share the same intent. The feature check runs the product endpoints and inspects structured evidence. The [RAG benchmark](README.md) measures ranking and generated answers separately.

## What constitutes evidence

| Surface | Evidence and grading |
| --- | --- |
| Branch files | Compare every published path and blob with the accepted Git tree at each observed tip. Refresh the same branch without changing its tip, then advance it to the next commit. Check the response, retained manifest and number of provider requests. Reopen each search result at its original revision and byte range. Report the rank of the frozen support and test an unpublished branch. |
| History | Compare parent ordering and tree identity with Git. For every changed file, compare both complete blob contents. Confirm the durable materialized evidence covers the expected line ranges and matches their Git bytes. Run the connector fixture for rename, deletion, root comparison and rejection of a second parent outside the lineage. |
| Graph | Report the full uv graph result with the number of accepted conventional Rust roots. The separate two-commit Git fixture checks a definition, an import and a call against original Git bytes, including a multibyte character before the call. Query an absent symbol and an unpublished branch. Remove the target module and verify that the next graph has no invented target. |
| Actions | Replay the recorded public attempt that reported the pinned head. Verify its exact attempt number, status, conclusion, repository and commit provenance. Reopen the definition at the pinned Git commit. The actual executed YAML is a separate unknown in the API. |
| Cross-feature | Require the current publication, history child and CI attempt to agree on the same immutable commit. Verify the parent independently against Git. This checks source consistency across endpoints. |

The uv source is the full pinned public Git tree. The provider fixture uses real objects and a captured public attempt. The test never executes repository code or a CI workflow. The bare repository has shallow ancestry, so the history benchmark checks the exact child and first parent. The fixture supplies complete controlled lineage for merge and root tests. Positive graph results are scoped to the small Rust fixture. There is no evidence about Java or cross-crate name resolution from this check.

## Natural questions across sources

[Family answers](family_answers.py) sends 28 bilingual questions through the same local API as Answers. It uses pinned Context Fabricator code and history, recorded GitHub Actions attempts from Context Fabricator and uv, and a two-commit Rust graph fixture. [Questions and evidence](family-answers-gold.json)

Git paths, commit IDs, source ranges and recorded attempt fields are checked independently of generation. For each response, the report preserves the question, answer, citations, expected evidence coverage and trace. The blind review grades factual correctness separately from source coverage. A correct abstention is required for private runtime state, missing revisions and attempts, unproven executed YAML, and a removed Rust definition. A structural trace event shows when graph-derived passages reached selection. These cases qualify the supported Rust graph scope and local answers over known publications.

Run the script with the Context Fabricator checkout, the local pinned uv object store, a debug app bundle, the qualified Q4 weight directory and a new report directory. It runs without opening the app or downloading a model.

## Reproduce without inference

Acquire the [pinned Git objects](../README.md#external-github-reference) and build a debug app bundle. Create a new output root, with `uv` and `graph` children reserved for the harness.

    bench=quality/github/uv_benchmark
    build=$(python3 -B quality/toolchain/cache-directory.py --build)
    bundle="$build/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
    out=$(mktemp -d)
    nice -n 15 python3 -B "$bench/features.py" "$reference_dir/uv.git" "$bundle" "$out/uv"
    nice -n 15 python3 -B "$bench/feature_graph.py" "$bundle" "$out/graph"

The packaged fixture transport serves REST objects over loopback. Its request counts measure the test path and agree with the backend ledger. Use the [Smart HTTP connector replay](../README.md#connector-replay) for Git pack sizes and exchange counts. Elapsed times here are single observations of a debug build, including HTTP and SQLite. They set no release latency threshold.
