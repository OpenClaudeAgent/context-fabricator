# External GitHub reference

The `astral-sh/uv` reference captures an exact Git commit, its first parent and a successful Actions attempt for that same commit. It is suitable for testing collection on a larger, independent Rust corpus. [Pinned reference](uv-reference.json) · [Source revision](https://github.com/astral-sh/uv/tree/a58e7cd639ef2ce0f4e6717b3e06269c3e1001ad) · [CI attempt](https://github.com/astral-sh/uv/actions/runs/44806099739/attempts/1)

From the repository root, fetch the pinned Git objects to a disposable bare repository. This uses public Git without credentials or a checkout.

```sh
reference_dir=$(mktemp -d)
git init --quiet --bare "$reference_dir/uv.git"
GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 \
  git -C "$reference_dir/uv.git" -c credential.helper= -c http.followRedirects=false \
  fetch --no-tags --depth=2 https://github.com/astral-sh/uv.git \
  a58e7cd639ef2ce0f4e6717b3e06269c3e1001ad
python3 -B quality/github/public_reference.py "$reference_dir/uv.git" "$reference_dir/offline"
```

Keep the bare repository to repeat the Python check offline. It verifies the Git tree, first parent, tracked path count and representative blobs. It writes `pinned-git.json` with the captured Actions metadata.

To recheck the recorded attempt, pass `--check-upstream` and a fresh report directory. The script reads the pinned Git commit and the exact attempt using two unauthenticated GitHub API requests. The upstream report is separate because GitHub retention can expire run metadata. The published test for Context Fabricator still uses `hosted.py` and its own repository.

## Connector replay

Use `--check-publication` with a fresh report directory to independently recompute the expected SQLite path-and-blob manifest from the pinned Git blobs. The reference expects 1,824 published paths. Binary files, files over the source limit and unsupported Git entries are skipped.

```bash
python3 -B quality/github/public_reference.py "$reference_dir/uv.git" "$reference_dir/oracle" --check-publication
source quality/toolchain/environment.sh
CTXFAB_UV_GIT_REFERENCE="$reference_dir/uv.git" \
  cargo test --manifest-path backend/Cargo.toml --locked --offline -p ctxfab-server --test github \
  git_fetch::external_reference_publishes_pinned_uv_files_without_per_file_requests \
  -- --exact --ignored --nocapture
```

The Rust test uses a disposable repository and database and a loopback Git server. It checks the staged publication and its commit, the persisted manifest, the request ledger, and the smart HTTP trace. The unchanged refresh verifies that the connector does not transfer another pack. The test never accesses GitHub or the Keychain and does not require a desktop session.

## RAG benchmark

The in-app **Inspect Pipeline** menu replays selected questions with the Rust answer pipeline. Use the harnesses below for frozen datasets, independent gold evidence and score comparisons. [Replay in the desktop](../../README.md#replay-a-benchmark)

The [uv benchmark](uv_benchmark/README.md) evaluates the full accepted repository with frozen English and French questions, exact Git support spans, paired retrieval comparisons and separate answer, citation and abstention judgments.

## Java branch development questions

The working-tree [Java development manifest](java_benchmark/development-cases.json) contains 22 previously exposed README and current-code requests on pinned Commons CLI and Jackson sources, including the evaluated method-ranking sample; it is a reproducible diagnostic, not a blind holdout. The original frozen evaluation and independent scoring remain separate evidence. With local bare Git stores containing the recorded commits and parents, the offline tool lists questions, verifies the Git trees, publication identities, exact source lines and explicit controls, and emits a gold-free one-repository projection for the headless Answers harness:

```sh
python3 -B quality/github/java_benchmark/development_cases.py list
python3 -B quality/github/java_benchmark/development_cases.py verify \
  --commons-git /path/to/commons-cli.git --jackson-git /path/to/jackson-databind.git
python3 -B quality/github/java_benchmark/development_cases.py project \
  --commons-git /path/to/commons-cli.git --repository commons-current > commons-requests.json
```

For a Jackson-only projection use `--repository jackson --jackson-git /path/to/jackson-databind.git`. Verification accepts `--repository` to check one store, or both stores without it. The `validation` branch in that projection is a local fixture name for the pinned Jackson commit, not a claim about its upstream branch. The file-absence control checks the full tracked tree; the README control checks the complete pinned file against its recorded numeric-version pattern. The runtime control checks that the cited method exists but its requested production observation is unavailable from source; it makes no claim about a live system. A regular-expression check is only the recorded absence test, not a general semantic proof of a negative. The command needs no ignored artifacts, network, credentials, or model; users provide the two local Git stores.

To run these exposed questions against a packaged macOS Debug app and the local pinned Q4 model, use a new report path and one repository at a time. This starts a local backend and one model worker in a temporary profile, with a loopback GitHub fixture and fixture-only credential. It does not open the app or access a real user profile:

```sh
python3 -B quality/github/java_benchmark/run_development.py \
  --repository commons-current --git /path/to/commons-cli.git \
  --bundle '/path/to/Context Fabricator.app' \
  --weights /path/to/Qwen3-4B-Instruct-2507-Q4_K_M.gguf \
  --report /path/to/new-report
```

Repeat `--case-id ID` to select explicit cases, for example a one-question smoke; unknown IDs fail. The report records selected IDs, binary/model hashes, full publication identity, each raw answer or API problem, exact citation reopening, timings and terminal unsubmitted cases. It refuses an existing report directory. The `validation` Jackson branch is emulated by the local fixture at the pinned commit. A literal reopened citation proves provenance, not that the answer follows from it. This is a single ordered diagnostic run, not a latency qualification or blind benchmark.

The separate [broad current-branch manifest](java_benchmark/broad-current-cases.json) preserves the original 16 exposed English/French current-branch matrix requests, their original opaque IDs and the oracle's conjunctive witness alternatives. It is not a new holdout. Pass `--manifest quality/github/java_benchmark/broad-current-cases.json` to `list`, `verify`, `project`, or `run_development.py`; the original oracle identity is recorded inside the manifest. For example, a single explicitly selected native smoke is:

```sh
python3 -B quality/github/java_benchmark/run_development.py \
  --manifest quality/github/java_benchmark/broad-current-cases.json \
  --repository commons-current --git /path/to/commons-cli.git \
  --bundle '/path/to/Context Fabricator.app' \
  --weights /path/to/Qwen3-4B-Instruct-2507-Q4_K_M.gguf \
  --case-id e684a92ac7ab25aba733 --report /path/to/new-broad-smoke
```

Verification checks every recorded byte span and all conjuncts of every alternative against its Git blob. The private-runtime, future-event and absent-namespace controls remain distinct: source can establish neither a private run nor a future event, and namespace verification checks the tracked Java files in this snapshot only. A source witness for a control provides context; it does not prove a production or future fact.
