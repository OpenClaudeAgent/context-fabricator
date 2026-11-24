# Context Fabricator screenshot gallery · macOS

Selected captures from the native SwiftUI preview. XCTest uses synthetic data and public benchmark revisions. The retry-delay answer comes from a fixture worker. Select any image to open it at full size. [Qt gallery](SCREENSHOTS-QT.md)

[Answers](#local-answers) · [Inspect Pipeline](#pipeline-inspection) · [Model management](#model-management) · [GitHub](#github) · [Diagnostics](#diagnostics) · [Overview and settings](#overview-and-settings)

## Local answers

Ask a question about the local corpus. The answer displays its supporting excerpt, and **View Trace** opens the steps behind the result.

[![Answers showing the retry-delay question, the local model, a source excerpt, and the View Trace button](screenshots/answers.png)](screenshots/answers.png)

[![Answer History with a saved retry-delay answer and its source excerpt](screenshots/answer-history.png)](screenshots/answer-history.png)

## Pipeline inspection

Open **Inspect Pipeline** from Answers to see which sources were searched, how candidates were ranked, what the model received and whether the citations passed verification. The Jackson example uses a pinned benchmark revision and shows the model input slots with their line ranges. [Pipeline architecture](../ARCHITECTURE.md#pipeline-inspection-and-benchmark-replay)

| Ranked candidates | Collapsed model input |
| --- | --- |
| [![Ranking for the Jackson benchmark showing a candidate, its file path and its model slot](screenshots/pipeline-ranking.png)](screenshots/pipeline-ranking.png) | [![Model Input showing five collapsed source slots and the exact completion input disclosure](screenshots/pipeline-model-input.png)](screenshots/pipeline-model-input.png) |

[![Citation Verification showing that the draft was withheld after quote verification failed](screenshots/pipeline-verification.png)](screenshots/pipeline-verification.png)

## Model management

**Manage Models** lists answer and embedding models, sizes and installation status. The macOS app bundles llama.cpp. The model library shows the engine build and controls for semantic indexing and stored embeddings. [Local inference](../ARCHITECTURE.md#local-inference)

| Model catalog | Inference and index controls |
| --- | --- |
| [![Manage Models showing Qwen3 4B answer profiles and the Qwen3 Embedding 0.6B profile with their download sizes](screenshots/model-library.png)](screenshots/model-library.png) | [![Model management showing llama.cpp Build 12359, a Semantic indexing switch and the Delete Index action](screenshots/model-runtime-and-index.png)](screenshots/model-runtime-and-index.png) |

## GitHub

Search repositories already loaded in the picker or enter a repository URL. Repository Settings shows the tracked branch, indexed commit and refresh action. The Requests panel shows the effective pace limit and outbound activity.

| Select a repository | Check freshness | Inspect request activity |
| --- | --- | --- |
| [![Repository picker with a search field, two fixture repositories, and a URL entry button](screenshots/github-picker.png)](screenshots/github-picker.png) | [![Repository Settings with the tracked branch, last indexed commit and Refresh Now button](screenshots/github-refresh.png)](screenshots/github-refresh.png) | [![GitHub Requests with the effective rate limit, concurrency and request counts](screenshots/github-requests.png)](screenshots/github-requests.png) |

[![GitHub Actions showing workflows and an observed run attempt for the fixture repository](screenshots/github-actions.png)](screenshots/github-actions.png)

## Diagnostics

Search logs, filter by level, or enable debug logging for five minutes. Open a correlated trace from a log entry or an answer to inspect operation names, timings, events, and related logs.

[![Diagnostics Requests showing the latest Answers request and its trace action](screenshots/diagnostics-requests.png)](screenshots/diagnostics-requests.png)

[![Diagnostics Logs showing the level selector and request entries](screenshots/diagnostics-logs.png)](screenshots/diagnostics-logs.png)

[![Trace inspector showing retrieval, generation and citation verification spans with the selected span's timing, events and logs](screenshots/trace-inspector.png)](screenshots/trace-inspector.png)

When citation verification fails, the answer trace shows the question, sources searched, candidate excerpt and reason for withholding the answer.

[![Answer trace for a withheld fixture response, showing the question, three search sources, candidate rank, original excerpt and failed citation verification](screenshots/trace-insufficient-evidence.png)](screenshots/trace-insufficient-evidence.png)

## Overview and settings

**Overview** reports corpus counts, local service readiness, and outbound activity for the current session.

[![Overview showing searchable and collected document counts, backend readiness, and outbound traffic](screenshots/overview.png)](screenshots/overview.png)

Use **Settings** to choose an answer language and adjust diagnostic retention and disk space. **Diagnostics → Storage** shows actual disk usage and lets you purge expired records.

| Settings | Diagnostic storage |
| --- | --- |
| [![Settings showing answer history, model storage, answer language and diagnostic retention](screenshots/settings.png)](screenshots/settings.png) | [![Storage showing bytes stored, the disk budget, retention and a Purge expired button](screenshots/diagnostics-storage.png)](screenshots/diagnostics-storage.png) |

[Native window tests](../desktop/macos/UITests/) · [Release preparation](RELEASE.md)
