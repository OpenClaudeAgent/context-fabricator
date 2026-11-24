# uv repository RAG benchmark

This benchmark asks the packaged Context Fabricator backend about the full accepted source tree of `astral-sh/uv` at `a58e7cd639ef2ce0f4e6717b3e06269c3e1001ad`. Its unit of analysis is a question intent. English and French versions share the same frozen facts and split. The two translations are paired observations. [Pinned uv commit](https://github.com/astral-sh/uv/tree/a58e7cd639ef2ce0f4e6717b3e06269c3e1001ad) · [Observed results](../../../.artifacts/evidence/uv-rag-benchmark/README.md)

## Scientific basis

- BEIR evaluates lexical and neural retrieval across heterogeneous tasks and establishes BM25 as a useful baseline. Here the CLI runs the same scoped queries against the same accepted corpus in BM25, semantic and hybrid modes when the full profile is selected. This single repository cannot estimate cross-repository performance. [Thakur et al., 2021](https://arxiv.org/abs/2104.08663)
- HotpotQA annotates supporting facts across documents. Here a question can require two separate facts located in different files. Returning the minimum Rust version does not answer the pinned-toolchain part. We report both fractional and complete evidence coverage. [Yang et al., 2018](https://aclanthology.org/D18-1259/)
- ALCE evaluates answer correctness and citation quality separately. We check the literal quote against Git, measure coverage of frozen support spans and provide a separate answer-content rubric. An authentic quote alone cannot establish that an answer is correct. [Gao et al., 2023](https://aclanthology.org/2023.emnlp-main.398/)

- SQuAD’s unanswerable extension pairs plausible questions with contexts that cannot establish their answers. Our private-state controls and excluded-file oracle adapt that principle to repository scope. These two private-state prompts are easier than adversarial crowdwritten questions. [Rajpurkar et al., 2018](https://aclanthology.org/P18-2124/)
- cAST studies the effects of respecting syntactic boundaries in code chunks. It motivates checking whether an evidence span is obtainable before interpreting a ranking miss. Our projection audit also covers prose and does not attribute cAST’s published improvements to this product. [Zhang et al., 2025](https://arxiv.org/abs/2506.15655)

| Adaptation | Execution evidence |
| --- | --- |
| Compare retrieval modes on the same queries and corpus | The reduced pilot ran 66 searches on the same 71 passages. Offline replay reopened all 1,297 hits against Git. [Ranks](../../../.artifacts/evidence/uv-rag-benchmark/pilot/rows.json) · [Replay](../../../.artifacts/evidence/uv-rag-benchmark/pilot/replay.json) |
| Judge multi-file evidence per fact | The toolchain question freezes separate workspace and developer toolchain facts. A duplicate hit cannot satisfy a different fact. [Judgments](gold.json) · [Scorer checks](test_score.py) |
| Verify citations and factual support separately | A response used an authentic requirement-display quotation for a question about cache identity. The blind reviewer marked the answer wrong after checking the quoted context. [Independent scores](../../../.artifacts/evidence/uv-rag-benchmark/full/review-wave-a-graded.json) |
| Observe correct abstentions | Both language versions of the two private-state controls were withheld without treating worker failure as a correct abstention. [Paired BM25 results](../../../.artifacts/evidence/uv-rag-benchmark/full/paired-answers-bm25.json) |
| Measure evidence lost to passage boundaries | The lock timeout spans two adjacent passages. The English generation run admitted both and verified a citation. The separate Tree-sitter comparison uses the same file slices and budgets for both chunking methods. [Full-corpus results](../../../.artifacts/evidence/uv-rag-benchmark/README.md) · [Syntax chunking experiment](../../../.artifacts/evidence/m07-structural-chunking/README.md) |

## Dataset and judgment

The benchmark author inspected immutable Git objects, selected target facts, wrote paired questions and froze all 11 intents before inspecting retrieval outputs. The indexing and answering systems receive only the question, repository ID and branch. Neither a retrieval result nor a generated answer was used to assign the gold facts. The labels were authored by an AI coding agent and verified against Git. They have no independent human adjudication. This version is public and unsuitable as a hidden test for future tuning. [Frozen questions and judgments](gold.json)

There are four development intents and seven held-out intents. Nine answerable intents contain 15 distinct required facts and 16 accepted support spans in eight files. Two controls request unobservable private workstation state. Family membership, both language variants, labels and alternatives stay on the same side of the split. Development and held-out subjects overlap at the repository level. This is a deliberately selected sample, with no claim that the prompts mirror user traffic.

The corpus contains genuine near-misses. The fast-ID documentation describes the 21-character comparator beside uv’s 16-character choice. The toolchain question distinguishes two real, different version values. The cache documentation gives global refresh, single-package refresh and reinstall options. A documented example index URL cannot establish a private configuration. A bare answer containing both numbers in the wrong roles fails the answer rubric.

The supplementary provenance oracles verify a before/after safety comment against two Git revisions, the exact upstream CI attempt conclusion, and an answer found only in a 3.8 MB excluded file. The coverage control searches the pinned tree for the entire digest and checks that no accepted path contains it. This benchmark scores generation over current code. The feature benchmark exercises the product APIs for branch updates, history, Actions and Rust code graph, with separate results. [Scope verifier](scopes.py) · [Feature benchmark](FEATURES.md)

## Measurement

1. Start a temporary app session with an in-memory fixture credential and a loopback GitHub REST origin serving the actual pinned Git objects. This uses the existing packaged fixture route. The separate Smart HTTP connector test measures production-style transfer. Record helper binary hashes, publication time, the source manifest and database footprint.
2. Reopen every current document against Git with path, blob, revision, bytes and original line numbers. Compare document IDs with FTS5's indexed document IDs. Check that passage extents cover each non-whitespace source byte without overlap, verify blob identity, summarize passage sizes and test whether gold anchors fit in an individual passage. This measures the production chunker. It does not compare against an alternative chunker.
3. Score the product's first 20 hits. Each fact can match any of its frozen equivalent spans and counts once regardless of duplicate hits. Report first rank per fact, reciprocal rank of the first fact, fraction of facts retrieved at 1/5/10/20, and whether all facts occur in the first five. Average within intent first, then across intents for each split/language/mode. Exclude unanswerable prompts from ranking denominators. Unjudged alternative evidence can be valid, so scores describe known anchors. No exhaustive relevance pool is assumed. The paired analysis reports right minus left mean recall@5 and an exploratory exact sign-flip statistic across matched held-out intents. [Scoring](score.py) · [Paired comparison](compare.py)
4. The generation profiles call the normal answer API with transient model context requested. Score only the excerpts actually returned in that context. Missing context is unknown. An abstention review’s candidate preview is not the completion payload. The scorer reopens every citation including its byte extent and verifies quote and line link. The anchor scores require full supporting spans. The separate review rubric permits a narrower, sufficient quote or independently valid evidence. Record the raw answer, trace identifier, answerability outcome, operational reason and latency. Worker, deadline, validation and context-limit failures receive no correct-abstention credit.
5. Grade each required fact in the answer against its role and polarity, then grade support from the cited quotations. An omitted fact counts as false. Mark contradictions, unsupported material additions and language appropriateness separately. Give full credit only for a complete answer with every fact supported and no unsupported additions or contradictions. Inspect the full original Git context when the wording admits another valid interpretation. The reviewer sheet shuffles order and hides mode and system identity. For an independent study, recruit separate blinded reviewers and retain independent decisions and disagreements before adjudication. Single-reviewer results should identify the reviewer type.

The timings include loopback IPC and CLI startup for search. Answer timings cover the API request. The fixed order and single sample per question permit descriptive latency summaries only. SQLite database and WAL sizes are point-in-time observations. Vector readiness is required for every published passage before comparing dense/hybrid results. Query-phase provider-attempt equality is checked against both the backend ledger and the origin request log.

## Reproduce

Use Python 3.12+ on macOS, a debug product bundle with the packaged helpers, and a fresh output directory for each run. Source collection and scoring are offline after obtaining the pinned bare repository as documented in the [external reference setup](../README.md). None of these commands opens the app. The model profiles use the bundle’s verified llama.cpp runtime, copy-on-write clones of local weights and an isolated product profile. The local fixture intercepts the model hub and the harness requires zero model download attempts.

```sh
bench=quality/github/uv_benchmark
build=$(python3 -B quality/toolchain/cache-directory.py --build)
bundle="$build/foundation-xcode/Build/Products/Debug/Context Fabricator.app"
assets="$(python3 -B quality/toolchain/cache-directory.py)/m09-assets"
git_objects="$reference_dir/uv.git"
out=$(mktemp -d)
python3 -B -m unittest discover -s "$bench"
nice -n 15 python3 -B "$bench/run.py" "$git_objects" "$bundle" "$out/bm25"
python3 -B "$bench/compare.py" "$out/bm25/rows.json" > "$out/bm25/paired.json"
nice -n 15 python3 -B "$bench/scopes.py" "$git_objects" \
  $bench/captured-attempt.json > "$out/scope-oracles.json"
nice -n 15 python3 -B "$bench/run.py" "$git_objects" "$bundle" "$out/smoke" --profile smoke --assets "$assets"
python3 -B "$bench/review.py" "$out/smoke/rows.json" "$out/review.json"
# Fill all boolean judgment fields in review.json, then validate and score it.
python3 -B "$bench/review.py" "$out/smoke/rows.json" "$out/assessment.json" --labels "$out/review.json"
```

The smoke selects four fixed prompts across the two languages and uses BM25 for generation. The full run indexes the entire corpus with Qwen3-Embedding-0.6B Q8_0, runs all 22 queries in each of the three retrieval modes and asks Qwen3-4B-Instruct-2507 Q4_K_M all 22 questions before and after embedding installation (BM25 then hybrid). Run the full profile on a host available for sustained inference.

```sh
nice -n 15 python3 -B "$bench/run.py" "$git_objects" "$bundle" "$out/full" --profile full --assets "$assets"
python3 -B "$bench/compare.py" "$out/full/rows.json" > "$out/full/paired.json"
```

The pinned SHA-256 digests are `06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439` for embeddings and `3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597` for generation. A model failure ends the run or is reported as the backend’s actual outcome. The harness never substitutes a proxy score.

## Reduced candidate-pool pilot

The optional pilot measures real embeddings on a judgment-enriched pool within a disposable copy of the published repository. All 28 paths, their blobs, roles and the selection algorithm were frozen before any pilot query. The manifest SHA-256 is `b9950589a7ff33f763dd943fec6cd8c5e332356a21ad14f4b9e5fd4c23e23eea`. It includes the eight source paths containing preregistered facts, twelve path-token distractors and eight hash-selected controls. The twenty extra files are at most 12 KiB each. Distractors are ranked by token overlap between their paths and the union of gold-path tokens after excluding common directory and extension tokens. Hash order breaks ties and selects controls. Neither question text, model output nor retrieval ranks participate. [Frozen pilot manifest](pilot-manifest.json) · [Selection and campaign](pilot.py)

The first helper publishes and verifies the complete original manifest. With that helper stopped, the harness deletes only out-of-pool search documents in the disposable SQLite projection. Product triggers update FTS. It preserves the complete source snapshot and Git revision. The second helper verifies and warms only the staged embedding model, waits for every candidate passage to have its vector, and runs the same 22 questions through all three search modes sequentially. It checks every hit against the original Git blob and candidate pool. The collection database and temporary model clone are removed after completion. The pilot fits a lower-compute diagnostic purpose. Absolute scores on its judgment-enriched pool are unsuitable estimates for the full corpus.

```sh
nice -n 15 python3 -B "$bench/pilot.py" "$git_objects" --freeze > "$out/regenerated-shard.json"
cmp "$bench/pilot-manifest.json" "$out/regenerated-shard.json"
nice -n 15 python3 -B "$bench/pilot.py" "$git_objects" \
  --bundle "$bundle" --assets "$assets" --report "$out/pilot"
for profile in bm25 smoke pilot; do
  python3 -B "$bench/replay.py" "$git_objects" "$out/$profile"
done
```

The full and pilot profiles both reopen and validate original evidence. The replay command independently rescores saved JSON without loading a model. To sample memory on macOS, run `resources.py BENCHMARK_PID` during the campaign. It samples the helper process tree once per second and reports maximum observed RSS per executable, excluding other apps. This measurement can miss brief peaks and includes shared pages.

## Full-corpus answer comparisons and expansion

The current full profile now generates **all 22 BM25 answers first**. It then adopts the local embedding weight, waits for complete indexing and generates **all 22 hybrid answers** on that same publication. Semantic-only is a retrieval comparator; the product answer endpoint decides which retrieval path it can actually use. The harness checks the response mode on every question. Timings include the product’s evidence selection, citation verification and refusals.

Reviewers receive shuffled sheets with questions, expected atomic facts, answer text, quoted evidence and source URLs. Assign reviewers independently of question authors and product changes. The review worksheet omits mode and measured retrieval ranks. A reviewer credits a materially correct fact conveyed with its proper role and polarity, withholds support credit for an unrelated quote, and marks extraneous factual claims. A lack of text gives zero answerable-fact recall. At reporting time, the product reason distinguishes a justified negative control from a worker, context or citation verification failure. Report complete grounded answers, false abstentions, correct abstentions, fact recall and language appropriateness by mode/split/language. The comparisons pair each intent across languages and modes; the 22 questions represent eleven intent clusters.

```sh
python3 -B "$bench/review.py" "$out/full/rows.json" "$out/bm25-blind.json" --mode bm25
python3 -B "$bench/review.py" "$out/full/rows.json" "$out/hybrid-blind.json" --mode hybrid
# Reviewers return the completed sheets unchanged except for judgment fields.
python3 -B "$bench/answer_pairs.py" "$out/full/rows.json" \
  --bm25 "$out/bm25-reviewed.json" --hybrid "$out/hybrid-reviewed.json" > "$out/answers-paired.json"
```

For future cross-language validation, Apache Commons CLI would provide a Java calibration corpus with command-line grammar, option definitions, parser behavior, errors and public usage examples. Jackson Databind is the separately selected Java holdout for mapping configuration, tree-model operations and relationships between parser, serializer and deserializer code. Both selected projects use Apache licensing; inspect the license and any vendored files at each pinned revision before enrollment. Count or cap accepted bytes/paths before enrollment, distinguish generated code, fixtures and Javadoc from production source, and measure answers against the precise requested source kind. Pin Git commit, parent, tags and any real Actions attempt with provenance before drafting gold. Neither Java candidate has a measured score in the uv campaign. [Commons CLI documentation](https://commons.apache.org/proper/commons-cli/) · [Jackson Databind repository](https://github.com/FasterXML/jackson-databind)

Freeze multiple repositories per implementation language with comparable sizes and domains before interpreting a language difference. Use separate repository-level development and blind holdout tiers. Preregister intent sampling, negatives, scope and admissible alternatives before any baseline search. Recruit independent maintainers or two blinded annotators to validate each atomic fact against the pinned source. Collect real contributor questions where possible. Compare systems within repository, report per-repository results and then macro-average repositories rather than letting the largest corpus dominate. Keep product graph extraction as its own eligibility criterion. Current code, diff, supported Actions metadata and citations can be assessed for Java, while the current graph parser specifically loads the Rust grammar. Java graph queries should report unsupported until a Java graph provider exists. [Current graph parser](../../../backend/server/src/domain/github/graph.rs)

## Reusing complete uv vectors

The full campaign’s optional checkpoint stores only the pinned repository commit/manifest, model profile, public source paths and blobs, passage byte extents, SHA-256 of the path/content input, and the 1,024-dimensional vectors. No source text, GGUF weight, credential, endpoint, question or answer belongs to that database. Its descriptor records its exact byte size and SHA-256. The evidence copy is read-only. [Exporter and guarded restore](checkpoint.py)

A fixture owner can call `restore_into_fixture(data, archive, expected_digest)` after publishing the exact original corpus in a fresh temporary profile and stopping its helper. The transaction requires an empty vector table and checks the complete expected path/blob manifest, each current passage input/extent, all vector sizes, profile identity and row counts. Changes to input text, title, chunk boundary, model profile or pinned corpus invalidate reuse. Report a restored experiment separately from first-time indexing throughput. The verification command exercises this operation against a temporary copy of a complete fixture and compares every restored payload with the archive. [Round-trip verifier](check_restore.py)

During a full run, `observe.py DATA REPORT` samples index progress and exports the sanitized checkpoint when all 11,188 vectors are ready. `traces.py DATA REPORT` retains actual answer telemetry. The DATA argument must point at the temporary benchmark profile. After completion, `stages.py REPORT` summarizes budgets, clipping and failure reasons. A recorded candidate preview alone does not establish its exact admitted text.

The Qwen oracle-context diagnostic replays each bilingual question with reference lines or the original indexed passages that intersect them. It reports raw drafts, literal quote copying, independent factual and support judgments, and the observed stage of each integrated answer failure. [Method, reproducible commands and measured outcomes](../../../.artifacts/evidence/uv-rag-benchmark/ORACLE-CONTEXT.md)
