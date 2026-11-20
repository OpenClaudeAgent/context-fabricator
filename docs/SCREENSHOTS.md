# Context Fabricator screenshot gallery

The current macOS preview, captured in native test sessions with synthetic repositories and data. The answer shown below comes from a fixture worker. Select any image to open it at full size.

[Answers](#local-answers) · [Model management](#model-management) · [GitHub](#github) · [Diagnostics](#diagnostics) · [Overview and settings](#overview-and-settings)

## Local answers

Ask a question about the local corpus. The answer displays its supporting excerpt, and **View Trace** opens the steps behind the result.

[![Answers showing the retry-delay question, the local model, a source excerpt, and the View Trace button](screenshots/answers.png)](screenshots/answers.png)

## Model management

**Manage Models** lists the available answer and embedding models, their download sizes, and their installation status. The preview uses a [manually installed inference runtime](../README.md#try-it-from-source).

[![Manage Models showing Qwen3 4B answer profiles and the Qwen3 Embedding 0.6B profile with their download sizes](screenshots/model-library.png)](screenshots/model-library.png)

## GitHub

Search repositories already loaded in the picker or enter a repository URL. Repository Settings shows the tracked branch, indexed commit, and refresh action. The Requests panel shows the effective pace limit and outbound activity.

| Select a repository | Check freshness | Inspect request activity |
| --- | --- | --- |
| [![Repository picker with a search field, two fixture repositories, and a URL entry button](screenshots/github-picker.png)](screenshots/github-picker.png) | [![Repository Settings with the tracked branch, last indexed commit and Refresh Now button](screenshots/github-refresh.png)](screenshots/github-refresh.png) | [![GitHub Requests with the effective rate limit, concurrency and request counts](screenshots/github-requests.png)](screenshots/github-requests.png) |

## Diagnostics

Search logs, filter by level, or enable debug logging for five minutes. Open a correlated trace from a log entry or an answer to inspect operation names, timings, events, and related logs.

[![Diagnostics showing the Debug log filter and entries for a search request and its lexical lookup](screenshots/diagnostics-logs.png)](screenshots/diagnostics-logs.png)

[![Trace inspector showing retrieval, generation and citation verification spans with the selected span's timing, events and logs](screenshots/trace-inspector.png)](screenshots/trace-inspector.png)

## Overview and settings

**Overview** reports corpus counts, local service readiness, and outbound activity for the current session.

[![Overview showing searchable and collected document counts, backend readiness, and outbound traffic](screenshots/overview.png)](screenshots/overview.png)

Use **Settings** to choose an answer language and adjust diagnostic retention and disk space. **Diagnostics → Storage** shows actual disk usage and lets you purge expired records.

| Settings | Diagnostic storage |
| --- | --- |
| [![Settings with answer language, diagnostic retention and a maximum disk space field](screenshots/settings.png)](screenshots/settings.png) | [![Storage showing bytes stored, the disk budget, retention and a Purge expired button](screenshots/diagnostics-storage.png)](screenshots/diagnostics-storage.png) |
