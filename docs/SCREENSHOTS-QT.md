# Context Fabricator screenshot gallery · Qt

The Qt Quick preview captured offscreen on macOS with the local Rust backend, synthetic repositories and a deterministic inference worker. Select any image to open it at full size. [Native macOS gallery](SCREENSHOTS.md)

[Answers](#local-answers) · [Inspect Pipeline](#pipeline-inspection) · [Model management](#model-management) · [GitHub](#github) · [Diagnostics](#diagnostics) · [Overview and settings](#overview-and-settings)

## Local answers

Ask about the local corpus, read the supporting source excerpt and open the corresponding trace. The history panel separates this session's questions from saved answers.

[![Qt Answers showing the retry-delay question, local model picker, answer and supporting excerpt](screenshots/qt/answers.png)](screenshots/qt/answers.png)

[![Compact French answer history showing published, withheld and cancelled requests](screenshots/qt/answer-history.png)](screenshots/qt/answer-history.png)

## Pipeline inspection

**Inspect Pipeline** puts corpus selection, retrieval, model input and verification in one window. The source panel lets you load pinned test corpora and public workflow runs. In **Benchmarks**, run a question or a suite and compare each answer with its expected result. The published status describes citation verification. [Pipeline architecture](../ARCHITECTURE.md#pipeline-inspection-and-benchmark-replay)

| Retrieved sources and candidates | Model input |
| --- | --- |
| [![Retrieval showing the sources searched, candidate counts and the source excerpt](screenshots/qt/pipeline-retrieval.png)](screenshots/qt/pipeline-retrieval.png) | [![Model Input showing the expanded source passage and the collapsed exact prompt with its copy action](screenshots/qt/pipeline-model-input.png)](screenshots/qt/pipeline-model-input.png) |

| Benchmark question set | Benchmark results |
| --- | --- |
| [![A pinned test suite showing code questions, a source link and a corpus download action](screenshots/qt/pipeline-benchmarks.png)](screenshots/qt/pipeline-benchmarks.png) | [![A completed test suite showing expected and model answers and a question with insufficient evidence](screenshots/qt/pipeline-results.png)](screenshots/qt/pipeline-results.png) |

## Model management

**Manage Models** displays generation and embedding profiles, installation sizes, setup and semantic indexing controls. [Local inference](../ARCHITECTURE.md#local-inference)

[![Qt Manage Models showing answer and embedding variants, download buttons and the semantic indexing switch](screenshots/qt/model-library.png)](screenshots/qt/model-library.png)

## GitHub

Find a repository in the picker or enter its URL. The repository list shows the tracked branch and collection state. Benchmark workflow downloads are accessible from **Inspect Pipeline → Sources**.

| Repository picker | Configured repositories |
| --- | --- |
| [![Repository picker with two synthetic repositories, a search field and a URL entry action](screenshots/qt/github-picker.png)](screenshots/qt/github-picker.png) | [![GitHub connector showing a filtered repository with its branch and Actions access state](screenshots/qt/github-repositories.png)](screenshots/qt/github-repositories.png) |

[![Public workflow downloads for pinned benchmark repositories with individual and bulk download buttons](screenshots/qt/pipeline-actions.png)](screenshots/qt/pipeline-actions.png)

## Diagnostics

Open the exact answer trace from **Requests** to inspect the evidence or switch to spans and logs. Requests distinguishes completed, interrupted and withheld answers.

| Requests | Answer trace |
| --- | --- |
| [![Diagnostics showing an insufficient-evidence result alongside earlier requests with trace links](screenshots/qt/diagnostics-requests.png)](screenshots/qt/diagnostics-requests.png) | [![Answer trace showing the verified citation and the source supplied to the model](screenshots/qt/trace-inspector.png)](screenshots/qt/trace-inspector.png) |

## Overview and settings

**Overview** shows the searchable corpus, backend readiness and outbound activity. **Settings** provides separate interface and answer languages, model management, answer history and diagnostic limits.

| Overview | Settings |
| --- | --- |
| [![Qt Overview showing an indexed fixture document and a ready backend](screenshots/qt/overview.png)](screenshots/qt/overview.png) | [![Qt Settings showing app language, answer settings and storage limits](screenshots/qt/settings.png)](screenshots/qt/settings.png) |

| Searchable answer language | Compact Sources in French |
| --- | --- |
| [![Answer language picker filtered to Japanese](screenshots/qt/answer-language.png)](screenshots/qt/answer-language.png) | [![French Sources screen at 440 pixels with download controls aligned to the right](screenshots/qt/pipeline-sources-compact-fr.png)](screenshots/qt/pipeline-sources-compact-fr.png) |

[Replay the window scenarios](../desktop/qt/README.md#rejouer-les-captures-de-la-fenêtre) · [Release preparation](RELEASE.md)
