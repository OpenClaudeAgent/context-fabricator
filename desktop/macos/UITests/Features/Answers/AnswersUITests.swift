import Foundation
import XCTest

@MainActor
final class AnswersUITests: XCTestCase {

  override func setUpWithError() throws {
    continueAfterFailure = false
  }

  private struct ModelCatalog: Decodable {
    let activeProfile: String?
  }

  func testPipelineShowsTheRejectedDraftAndItsContext() async throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory, hybrid: true)
    try seedAnswerCorpus(in: directory)
    try await enableHybridIndex(in: BackendEndpoint.read(in: directory))
    app.selectWorkspace("answers")
    let inspector = app.buttons["open-answer-pipeline"]
    XCTAssertTrue(inspector.waitForExistence(timeout: 5))
    inspector.click()
    let question = app.textFields["answer-pipeline-question"]
    XCTAssertTrue(question.waitForExistence(timeout: 5))
    question.click()
    question.typeText("Fabri")
    question.typeKey("c", modifierFlags: [])
    question.typeText("ate 47")
    XCTAssertEqual(question.value as? String, "Fabricate 47")
    let composerFrame = question.frame
    let runFrame = app.buttons["answer-pipeline-run"].frame
    question.typeKey(.return, modifierFlags: [])
    let verification = app.buttons["answer-pipeline-stage-verification"]
    XCTAssertTrue(verification.waitForExistence(timeout: 5))
    verification.click()
    XCTAssertTrue(app.staticTexts["answer-pipeline-withheld"].waitForExistence(timeout: 10))
    XCTAssertEqual(question.frame, composerFrame)
    XCTAssertEqual(app.buttons["answer-pipeline-run"].frame, runFrame)
    assertTraceText(
      "The draft’s citations did not pass verification against the indexed sources.",
      identifier: "answer-pipeline-reason", in: app)
    captureWindow("Native Answers pipeline - verification", in: app)
    app.buttons["answer-pipeline-stage-context"].click()
    let input = app.staticTexts["answer-pipeline-input-1"]
    XCTAssertFalse(input.exists)
    let slot = app.descendants(matching: .any).matching(identifier: "answer-pipeline-input-slot-1")
      .firstMatch
    XCTAssertTrue(slot.waitForExistence(timeout: 5))
    slot.click()
    XCTAssertTrue(input.waitForExistence(timeout: 5))
    XCTAssertTrue(traceText(input).contains("47 seconds"))
    app.buttons["answer-pipeline-stage-generation"].click()
    XCTAssertTrue(app.staticTexts["answer-pipeline-draft"].exists)
    captureWindow("Native Answers pipeline - rejected draft", in: app)
    verifyPipelineEmbedding(in: app)
    app.buttons["answer-pipeline-close"].click()
    XCTAssertTrue(question.waitForNonExistence(timeout: 5))
  }

  private func verifyPipelineEmbedding(in app: XCUIApplication) {
    app.buttons["answer-pipeline-stage-embedding"].click()
    XCTAssertTrue(app.staticTexts["Nearest passages"].waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["Retry cooldown"].exists)
    XCTAssertFalse(app.buttons["View Retrieval"].exists)
  }

  func testPipelineBenchmarksRunAgainstPastedText() throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory)
    app.selectWorkspace("answers")
    app.buttons["open-answer-pipeline"].click()
    XCTAssertTrue(app.buttons["answer-pipeline-stage-index"].exists)
    XCTAssertTrue(app.buttons["answer-pipeline-stage-embedding"].exists)
    try checkBenchmarkSources(in: app)
    pasteBenchmarkDocument(in: app)
    let question = app.textFields["answer-pipeline-question"]
    question.click()
    question.typeText("Jetmarker")
    XCTAssertEqual(question.value as? String, "Jetmarker")
    question.typeKey(.return, modifierFlags: [])
    app.buttons["answer-pipeline-stage-verification"].click()
    let result = app.staticTexts.matching(
      NSPredicate(format: "value == %@", "Jetmarker waits 73 ms.")
    ).firstMatch
    XCTAssertTrue(result.waitForExistence(timeout: 10))
    captureWindow("Native Answers benchmarks - verified answer", in: app)
    app.buttons["answer-pipeline-stage-corpus"].click()
    let file = app.staticTexts.matching(NSPredicate(format: "value == %@", "note.txt")).firstMatch
    XCTAssertTrue(file.waitForExistence(timeout: 5))
    captureWindow("Native Answers benchmarks - temporary corpus", in: app)
    verifyPastedCorpusStages(in: app)
  }

  private func verifyPastedCorpusStages(in app: XCUIApplication) {
    app.buttons["answer-pipeline-stage-index"].click()
    XCTAssertTrue(app.staticTexts["SQLite FTS5"].exists)
    XCTAssertTrue(app.staticTexts["BM25"].exists)
    app.buttons["answer-pipeline-stage-embedding"].click()
    XCTAssertTrue(
      app.staticTexts["This run used keyword search. BM25 does not calculate a query embedding."]
        .exists)
    app.buttons["View Retrieval"].click()
    openRanking(for: app.buttons["pipeline-retrieval-candidate-0"], in: app)
    XCTAssertTrue(app.buttons["pipeline-ranking-candidate-0"].isSelected)
    app.buttons["pipeline-ranking-open-model-slot"].click()
    let passage = app.staticTexts["answer-pipeline-input-1"]
    XCTAssertTrue(passage.waitForExistence(timeout: 5))
    XCTAssertTrue(traceText(passage).contains("Jetmarker waits 73 ms."))
  }

  func testBenchmarkRunnerReplaysRecordedWorkflowWithoutDownloading() throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory)
    app.selectWorkspace("answers")
    app.buttons["open-answer-pipeline"].click()
    openPipelineSources(in: app)
    app.buttons["pipeline-manage-sources"].click()
    let recorded = app.buttons["Recorded Captures"]
    XCTAssertTrue(recorded.wait(for: \.isHittable, toEqual: true, timeout: 5))
    recorded.click()
    let load = app.buttons["Load Recorded Captures"]
    XCTAssertTrue(load.wait(for: \.isHittable, toEqual: true, timeout: 5))
    load.click()
    XCTAssertTrue(load.waitForNonExistence(timeout: 5))
    let done = app.buttons["pipeline-manage-sources-done"]
    XCTAssertTrue(done.wait(for: \.isHittable, toEqual: true, timeout: 5))
    done.click()
    XCTAssertTrue(done.waitForNonExistence(timeout: 5))

    app.buttons["answer-pipeline-benchmarks"].click()
    XCTAssertTrue(
      app.staticTexts["What was the outcome of the selected CI workflow?"]
        .firstMatch.waitForExistence(timeout: 5))
    let run = app.buttons["benchmark-run-question"]
    XCTAssertTrue(run.wait(for: \.isEnabled, toEqual: true, timeout: 5))
    run.click()
    let inspect = app.buttons["benchmark-inspect-result"]
    XCTAssertTrue(inspect.wait(for: \.isEnabled, toEqual: true, timeout: 12))
    let answer = app.staticTexts.matching(
      NSPredicate(format: "value == %@", "Conclusion: cancelled")
    ).firstMatch
    XCTAssertTrue(answer.exists)
    inspect.click()
    XCTAssertTrue(app.buttons["answer-pipeline-stage-retrieval"].isSelected)
    app.buttons["answer-pipeline-stage-embedding"].click()
    XCTAssertTrue(app.staticTexts["Exact workflow attempt"].exists)
    app.buttons["answer-pipeline-stage-verification"].click()
    XCTAssertTrue(answer.exists)
  }

  func testPinnedBenchmarkCorpusCanBeReactivatedAfterRestart() async throws {
    let environment = ProcessInfo.processInfo.environment
    let port = try XCTUnwrap(environment["CTXFAB_BENCHMARK_GIT_PORT"])
    let revision = try XCTUnwrap(environment["CTXFAB_BENCHMARK_GIT_REVISION"])
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_GITHUB_FIXTURE_PORT"] = port
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory, hybrid: true)
    let endpoint = try BackendEndpoint.read(in: directory)
    try await setEmbeddingIndex(false, in: endpoint)
    try await fetchPinnedCorpus(revision: revision, in: endpoint)
    try openPinnedCorpus(revision: revision, in: app)
    let ready = app.descendants(matching: .any).matching(identifier: "benchmark-reference-ready")
      .firstMatch
    _ = try XCTUnwrap(
      ready.waitForExistence(timeout: 5) ? ready : nil, "The selected corpus should be ready.")
    app.buttons["pipeline-manage-sources"].click()
    let activations = app.descendants(matching: .any)
      .matching(identifier: "pipeline-active-corpus-fixture/alpha-\(revision)")
    let activation = activations.firstMatch
    try revealPipelineControl(activation, in: app.scrollViews["pipeline-source-manager-content"])
    XCTAssertEqual(activation.value as? Int, 1)
    activation.click()
    let inactive = activations.matching(NSPredicate(format: "value == 0")).firstMatch
    XCTAssertTrue(inactive.waitForExistence(timeout: 5))

    app.terminate()
    app.launch()
    try openPinnedCorpus(revision: revision, in: app)
    let activate = app.buttons["benchmark-load-reference"]
    _ = try XCTUnwrap(activate.wait(for: \.isHittable, toEqual: true, timeout: 5) ? activate : nil)
    XCTAssertEqual(activate.label, "Activate")
    activate.click()
    XCTAssertTrue(ready.waitForExistence(timeout: 10))
    dismissPipelineSources(in: app)
    try await verifyPinnedCorpusSearch(in: app, endpoint: BackendEndpoint.read(in: directory))
  }

  private func verifyPinnedCorpusSearch(in app: XCUIApplication, endpoint: BackendEndpoint)
    async throws
  {
    let question = app.textFields["answer-pipeline-question"]
    question.click()
    question.typeText("Driftmarker")
    question.typeKey(.return, modifierFlags: [])
    let fallback = app.buttons["Use BM25"]
    XCTAssertTrue(fallback.wait(for: \.isHittable, toEqual: true, timeout: 10))
    fallback.click()
    app.buttons["answer-pipeline-run"].click()
    app.buttons["answer-pipeline-stage-verification"].click()
    let answer = app.staticTexts.matching(
      NSPredicate(format: "value == %@", "Driftmarker waits 108 ms.")
    ).firstMatch
    XCTAssertTrue(answer.waitForExistence(timeout: 10))

    try await setEmbeddingIndex(true, in: endpoint)
    openPipelineSources(in: app)
    selectBenchmarkSearchMode("Hybrid", in: app)
    dismissPipelineSources(in: app)
    app.buttons["answer-pipeline-run"].click()
    app.buttons["answer-pipeline-stage-embedding"].click()
    XCTAssertTrue(app.staticTexts["Nearest passages"].waitForExistence(timeout: 12))
    XCTAssertTrue(app.staticTexts["queue.txt"].exists)
  }

  private func fetchPinnedCorpus(revision: String, in endpoint: BackendEndpoint) async throws {
    struct Corpus: Decodable {
      let files: Int
      let revision: String
    }
    var request = URLRequest(url: endpoint.url.appending(path: "v1/benchmarks/corpus"))
    request.httpMethod = "POST"
    request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.httpBody = try JSONEncoder().encode([
      "repository": "fixture/alpha", "revision": revision,
    ])
    let (body, response) = try await URLSession.shared.data(for: request)
    XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
    let corpus = try JSONDecoder().decode(Corpus.self, from: body)
    XCTAssertEqual(corpus.files, 1)
    XCTAssertEqual(corpus.revision, revision)
  }

  private func openPinnedCorpus(revision: String, in app: XCUIApplication) throws {
    let ready = app.descendants(matching: .any).matching(identifier: "backend-ready").firstMatch
    XCTAssertTrue(ready.waitForExistence(timeout: 12))
    app.selectWorkspace("answers")
    let inspector = app.buttons["open-answer-pipeline"]
    XCTAssertTrue(inspector.wait(for: \.isHittable, toEqual: true, timeout: 8))
    inspector.click()
    openPipelineSources(in: app)
    let corpus = app.buttons["benchmark-corpus-fixture/alpha-\(revision)"]
    try revealPipelineControl(corpus, in: app.scrollViews["pipeline-source-list"])
    corpus.click()
  }

  private func revealPipelineControl(_ control: XCUIElement, in scroll: XCUIElement) throws {
    _ = try XCTUnwrap(scroll.waitForExistence(timeout: 5) ? scroll : nil)
    _ = try XCTUnwrap(control.waitForExistence(timeout: 5) ? control : nil)
    if !scroll.frame.contains(control.frame) {
      scroll.scroll(byDeltaX: 0, deltaY: -400)
    }
    _ = try XCTUnwrap(
      control.wait(for: \.isHittable, toEqual: true, timeout: 5)
        && scroll.frame.contains(control.frame) ? control : nil,
      "The source control must be visible inside its scroll view before clicking.")
  }

  private func checkBenchmarkSources(in app: XCUIApplication) throws {
    openPipelineSources(in: app)
    let manage = app.buttons["pipeline-manage-sources"]
    _ = try XCTUnwrap(
      manage.wait(for: \.isEnabled, toEqual: true, timeout: 5) ? manage : nil)
    manage.click()
    for repository in ["FasterXML/jackson-databind", "tokio-rs/bytes"] {
      let item = app.staticTexts[repository].firstMatch
      XCTAssertTrue(item.waitForExistence(timeout: 4), "Benchmark corpus \(repository) is missing.")
    }
    let done = app.buttons["pipeline-manage-sources-done"]
    _ = try XCTUnwrap(done.wait(for: \.isHittable, toEqual: true, timeout: 5) ? done : nil)
    done.click()
    XCTAssertTrue(done.waitForNonExistence(timeout: 5))
  }

  private func openPipelineSources(in app: XCUIApplication) {
    let sources = app.buttons["answer-pipeline-scope"]
    XCTAssertTrue(sources.wait(for: \.isHittable, toEqual: true, timeout: 5))
    sources.click()
    XCTAssertTrue(
      app.buttons["pipeline-manage-sources"].wait(for: \.isEnabled, toEqual: true, timeout: 5))
  }

  private func dismissPipelineSources(in app: XCUIApplication) {
    let manage = app.buttons["pipeline-manage-sources"]
    if manage.exists {
      app.typeKey(.escape, modifierFlags: [])
      XCTAssertTrue(manage.waitForNonExistence(timeout: 5))
    }
    XCTAssertTrue(app.textFields["answer-pipeline-question"].exists)
  }

  private func selectBenchmarkSearchMode(_ title: String, in app: XCUIApplication) {
    let mode = app.descendants(matching: .any).matching(identifier: "benchmark-retrieval-mode")
      .firstMatch
    XCTAssertTrue(mode.wait(for: \.isHittable, toEqual: true, timeout: 5))
    mode.click()
    app.menuItems[title].click()
  }

  private func pasteBenchmarkDocument(in app: XCUIApplication) {
    openPipelineSources(in: app)
    let imports = app.descendants(matching: .any).matching(
      identifier: "answer-pipeline-import-corpus"
    )
    .firstMatch
    XCTAssertTrue(imports.waitForExistence(timeout: 5))
    imports.click()
    let paste = app.menuItems["Paste Text…"]
    XCTAssertTrue(paste.exists)
    paste.click()
    let path = app.textFields["benchmark-document-path"]
    XCTAssertTrue(path.waitForExistence(timeout: 5))
    path.click()
    path.typeText("note.txt")
    let text = app.textViews["benchmark-document-text"]
    XCTAssertTrue(text.exists)
    text.click()
    text.typeText("Jetmarker waits 73 ms.")
    app.buttons["benchmark-add-document"].click()
    XCTAssertTrue(path.waitForNonExistence(timeout: 5))
  }

  func testPinnedJacksonCorpusLoadsAndReplaysInInspector() throws {
    try XCTSkipUnless(ProcessInfo.processInfo.environment["CTXFAB_XCTEST_LIVE_BENCHMARK"] == "1")
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory)
    app.selectWorkspace("answers")
    app.buttons["open-answer-pipeline"].click()
    openPipelineSources(in: app)
    let jackson = app.buttons[
      "benchmark-corpus-FasterXML/jackson-databind-c92512e95ba389d50a9ff6278906eff24ab3128e"]
    XCTAssertTrue(jackson.wait(for: \.isHittable, toEqual: true, timeout: 5))
    jackson.click()
    let download = app.buttons["benchmark-load-reference"]
    XCTAssertTrue(download.wait(for: \.isHittable, toEqual: true, timeout: 5))
    download.click()
    let ready = app.descendants(matching: .any).matching(identifier: "benchmark-reference-ready")
      .firstMatch
    XCTAssertTrue(ready.waitForExistence(timeout: 35))
    selectBenchmarkSearchMode("BM25", in: app)
    dismissPipelineSources(in: app)
    app.descendants(matching: .any).matching(identifier: "benchmark-questions-for-corpus")
      .firstMatch.click()
    app.menuItems["benchmark-case-ArrayNode.remove(-1)"].click()
    let question = app.textFields["answer-pipeline-question"]
    XCTAssertEqual(
      question.value as? String,
      "In Jackson, what does ArrayNode.remove(-1) return? Does it remove the last element?")
    XCTAssertTrue(app.buttons["answer-pipeline-run"].isEnabled)
    captureWindow("Native benchmark - Jackson loaded", in: app)
    app.buttons["answer-pipeline-run"].click()
    app.buttons["answer-pipeline-stage-ranking"].click()
    let evidence = app.buttons.matching(
      NSPredicate(format: "label CONTAINS %@", "ArrayNode.java")
    ).firstMatch
    XCTAssertTrue(evidence.waitForExistence(timeout: 12))
    evidence.click()
    let openSlot = app.buttons["pipeline-ranking-open-model-slot"]
    XCTAssertTrue(openSlot.exists)
    let slot = try XCTUnwrap(openSlot.label.split(separator: " ").last.flatMap { Int($0) })
    let passage = app.staticTexts["answer-pipeline-input-\(slot)"]
    captureWindow("Native benchmark - Jackson ranking", in: app)
    app.buttons["answer-pipeline-stage-context"].click()
    XCTAssertFalse(passage.exists)
    captureWindow("Native benchmark - Jackson model input folded", in: app)
    app.buttons["answer-pipeline-stage-ranking"].click()
    openSlot.click()
    XCTAssertTrue(passage.waitForExistence(timeout: 5))
  }

  func testCommonsCLIBenchmarkShowsRetrievedPassagesAndBM25Reason() throws {
    try XCTSkipUnless(ProcessInfo.processInfo.environment["CTXFAB_XCTEST_LIVE_BENCHMARK"] == "1")
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory)
    app.selectWorkspace("answers")
    app.buttons["open-answer-pipeline"].click()
    let editor = app.textFields["answer-pipeline-question"]
    let run = app.buttons["answer-pipeline-run"]
    let initialEditorFrame = editor.frame
    let initialRunFrame = run.frame
    openPipelineSources(in: app)
    app.buttons["benchmark-corpus-apache/commons-cli-aec9bb2dd90df2d7f775d6da2f026a1c328a03e6"]
      .click()
    let download = app.buttons["benchmark-load-reference"]
    XCTAssertTrue(download.waitForExistence(timeout: 5))
    XCTAssertEqual(editor.frame, initialEditorFrame)
    XCTAssertEqual(run.frame, initialRunFrame)
    download.click()
    let ready = app.descendants(matching: .any).matching(identifier: "benchmark-reference-ready")
      .firstMatch
    XCTAssertTrue(ready.waitForExistence(timeout: 35))
    XCTAssertEqual(editor.frame, initialEditorFrame)
    XCTAssertEqual(run.frame, initialRunFrame)
    selectBenchmarkSearchMode("BM25", in: app)
    dismissPipelineSources(in: app)
    captureWindow("Commons CLI · corpus ready", in: app)
    let questions = app.descendants(matching: .any)
      .matching(identifier: "benchmark-questions-for-corpus").firstMatch
    questions.click()
    app.menuItems["benchmark-case-Quote cleanup"].click()
    let question = app.textFields["answer-pipeline-question"].value as? String
    XCTAssertTrue(question?.hasPrefix("In Commons CLI, what does quote stripping return") == true)
    app.buttons["answer-pipeline-run"].click()
    app.buttons["answer-pipeline-stage-retrieval"].click()
    let candidate = app.buttons.matching(
      NSPredicate(format: "identifier BEGINSWITH %@", "pipeline-retrieval-candidate-")
    ).firstMatch
    XCTAssertTrue(candidate.waitForExistence(timeout: 12))
    captureWindow("Commons CLI · retrieved candidates", in: app)
    openRanking(for: candidate, in: app)
    XCTAssertTrue(app.buttons["answer-pipeline-stage-ranking"].isSelected)
    app.buttons["answer-pipeline-stage-embedding"].click()
    XCTAssertTrue(app.buttons["View Retrieval"].exists)
    captureWindow("Commons CLI · BM25 without embeddings", in: app)
  }

  private func openRanking(for candidate: XCUIElement, in app: XCUIApplication) {
    XCTAssertTrue(candidate.wait(for: \.isHittable, toEqual: true, timeout: 5))
    candidate.click()
    let ranking = app.buttons.matching(
      NSPredicate(format: "identifier BEGINSWITH %@", "pipeline-retrieval-view-ranking-")
    ).firstMatch
    XCTAssertTrue(ranking.wait(for: \.isHittable, toEqual: true, timeout: 5))
    XCTAssertEqual(candidate.value as? String, "Expanded")
    ranking.click()
  }

  func testAbstentionTraceShowsSearchedSourcesAndCandidateDisposition() throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory)
    try seedAnswerCorpus(in: directory)
    let noMatch = "orbitalzebramarker"
    openAbstentionTrace(noMatch, in: app)
    assertTraceText(
      "The search returned zero candidates. Answer generation was skipped.",
      identifier: "trace-evidence-reason", in: app)
    XCTAssertTrue(app.staticTexts["trace-evidence-no-candidates"].exists)
    assertTraceText("0 candidates", identifier: "trace-evidence-source-count-0", in: app)
    assertSearchedSourceAlignment(in: app)
    captureWindow("Insufficient evidence - zero candidates", in: app)
    app.buttons["trace-inspector-close"].click()
    XCTAssertTrue(app.staticTexts["trace-insufficient-evidence"].waitForNonExistence(timeout: 5))

    openAbstentionTrace("Fabricate cooldown", in: app)
    assertTraceText("src/outbound.rs", identifier: "trace-evidence-candidate-0", in: app)
    assertTraceText(
      "Cited by the draft. Verification failed for the draft’s citations.",
      identifier: "trace-evidence-disposition-0", in: app)
    captureWindow("Insufficient evidence - citation rejected", in: app)
    app.radioGroups["trace-inspector-pane"].radioButtons["Spans"].click()
    assertTraceText("Fabricate cooldown", identifier: "trace-request-question", in: app)
    assertTraceText(
      "Insufficient evidence", identifier: "trace-span-outcome", in: app)
    app.buttons["trace-inspector-close"].click()
    verifyEarlierSessionQuestion(noMatch, in: app)
  }

  private func assertSearchedSourceAlignment(in app: XCUIApplication) {
    assertTraceText(
      "Keyword search (BM25)", identifier: "trace-evidence-source-detail-0", in: app)
    XCTAssertFalse(app.staticTexts["trace-evidence-source-detail-1"].exists)
    XCTAssertFalse(app.staticTexts["trace-evidence-source-detail-2"].exists)
    let counts = (0..<3).map { app.staticTexts["trace-evidence-source-count-\($0)"] }
    for count in counts {
      XCTAssertTrue(count.exists)
      XCTAssertFalse(count.frame.isEmpty)
    }
    XCTAssertEqual(counts[0].frame.maxX, counts[1].frame.maxX, accuracy: 2)
    XCTAssertEqual(counts[1].frame.maxX, counts[2].frame.maxX, accuracy: 2)
    XCTAssertEqual(
      counts[1].frame.midY - counts[0].frame.midY, counts[2].frame.midY - counts[1].frame.midY,
      accuracy: 2)
  }

  private func verifyEarlierSessionQuestion(_ question: String, in app: XCUIApplication) {
    app.selectWorkspace("answers")
    app.buttons["answer-history"].click()
    let entry = app.buttons.matching(
      NSPredicate(format: "identifier BEGINSWITH %@ AND label == %@", "session-question-", question)
    ).firstMatch
    XCTAssertTrue(entry.waitForExistence(timeout: 5))
    entry.click()
    let panes = app.radioGroups["trace-inspector-pane"]
    XCTAssertTrue(panes.waitForExistence(timeout: 5))
    panes.radioButtons["Spans"].click()
    assertTraceText(question, identifier: "trace-request-question", in: app)
  }

  private func openAbstentionTrace(_ question: String, in app: XCUIApplication) {
    app.selectWorkspace("answers")
    let editor = app.textViews["answer-question"]
    XCTAssertTrue(editor.waitForExistence(timeout: 5))
    editor.click()
    editor.typeKey("a", modifierFlags: .command)
    for character in question {
      editor.typeKey(String(character), modifierFlags: [])
    }
    XCTAssertEqual(editor.value as? String, question)
    editor.typeKey(.return, modifierFlags: [])
    XCTAssertTrue(app.buttons["view-answer-trace"].waitForExistence(timeout: 8))
    app.selectWorkspace("diagnostics")
    assertTraceText(question, identifier: "diagnostic-latest-answer-question", in: app)
    app.buttons["diagnostic-view-latest-answer-trace"].click()
    XCTAssertTrue(app.staticTexts["trace-insufficient-evidence"].waitForExistence(timeout: 5))
    assertTraceText(question, identifier: "trace-evidence-question", in: app)
  }

  func testAnswersSwitchesModelWithoutRestartingAndShowsInsufficientEvidence() async throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    let assets = try launchAnswerFixture(in: app, directory: directory)
    let warmupGate = assets.appending(path: "Qwen3-4B-Instruct-2507-Q5_K_M.startup-gate")
    try Data().write(to: warmupGate)
    let original = try BackendEndpoint.read(in: directory)
    app.selectWorkspace("answers")
    let picker = app.descendants(matching: .any).matching(identifier: "generation-model-picker")
      .firstMatch
    XCTAssertTrue(picker.waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["Ask about your corpus"].exists)
    let question = app.textViews["answer-question"]
    enterTwoQuestionLines(in: question)
    let ask = app.buttons["Ask locally"]
    XCTAssertTrue(ask.isEnabled)
    captureWindow("Answers model picker", in: app)
    picker.click()
    app.menuItems["Qwen3 4B · Q5_K_M"].click()
    let loading = app.staticTexts["generation-model-loading"]
    XCTAssertTrue(loading.waitForExistence(timeout: 4))
    XCTAssertEqual(loading.value as? String ?? loading.label, "Loading Qwen3 4B · Q5_K_M…")
    XCTAssertEqual(picker.label, "Local model, Qwen3 4B · Q4_K_M")
    XCTAssertTrue(picker.isEnabled)
    XCTAssertFalse(ask.isEnabled)
    captureWindow("Answers model loading", in: app)
    verifyBusyModelMenu(picker, in: app)
    try FileManager.default.removeItem(at: warmupGate)
    try await verifyAlternateProfile(in: original, app: app, assets: assets)
    XCTAssertTrue(loading.waitForNonExistence(timeout: 5))
    XCTAssertEqual(picker.label, "Local model, Qwen3 4B · Q5_K_M")
    XCTAssertTrue(picker.isEnabled)
    XCTAssertTrue(ask.isEnabled)
    XCTAssertEqual(question.value as? String, "First line\nMore detail")
    captureWindow("Answers model activated", in: app)
    submitQuestionFromKeyboard(in: app)
    try assertSameSession(as: original, in: directory)
  }

  func testAnswerUsageShowsPublishedDesktopAnswerAndOpensItsTrace() throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory)
    try seedAnswerCorpus(in: directory)
    app.selectWorkspace("answers")
    enterLocalQuestion("What is the retry delay?", in: app)
    verifyCitedAnswer(in: app)
    let expectedTrace = publishedRequestTrace(in: app)
    let trace = app.staticTexts["trace-inspector-id"]
    let tab = app.radioGroups["diagnostic-tabs"].radioButtons["Answer Usage"]
    XCTAssertTrue(tab.waitForExistence(timeout: 5))
    tab.click()
    let client = app.staticTexts["answer-usage-client-0"]
    XCTAssertTrue(client.waitForExistence(timeout: 8), app.debugDescription)
    XCTAssertEqual(client.value as? String ?? client.label, "macOS")
    let profile = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"
    let model = app.staticTexts["answer-usage-model-0"]
    XCTAssertEqual(model.value as? String ?? model.label, profile)
    let published = app.staticTexts["answer-usage-published-0"]
    XCTAssertEqual(published.value as? String ?? published.label, "1")
    captureWindow("Diagnostics answer usage", in: app)
    let window = app.windows.firstMatch
    narrowWindow(window)
    XCTAssertEqual(traceText(published), "1")
    XCTAssertFalse(published.frame.isEmpty)
    XCTAssertTrue(window.frame.contains(published.frame))
    XCTAssertTrue(app.buttons["answer-usage-refresh"].isHittable)
    XCTAssertTrue(window.frame.contains(tab.frame))
    let latest = app.buttons["answer-usage-trace-0"]
    XCTAssertTrue(latest.isHittable)
    XCTAssertTrue(window.frame.contains(latest.frame))
    captureWindow("Diagnostics compact answer usage", in: app)
    latest.click()
    XCTAssertTrue(trace.waitForExistence(timeout: 5))
    XCTAssertEqual(trace.value as? String ?? trace.label, expectedTrace)
    app.radioGroups["trace-inspector-pane"].radioButtons["Spans"].click()
    XCTAssertTrue(app.staticTexts["trace-step-api_answer"].waitForExistence(timeout: 5))
    assertTraceText("macOS", identifier: "trace-span-client", in: app)
    assertTraceText(profile, identifier: "trace-span-model", in: app)
    captureWindow("Diagnostics answer usage trace", in: app)
  }

  private func publishedRequestTrace(in app: XCUIApplication) -> String {
    app.selectWorkspace("diagnostics")
    let latestQuestion = app.staticTexts["diagnostic-latest-answer-question"]
    XCTAssertTrue(latestQuestion.waitForExistence(timeout: 5), app.debugDescription)
    XCTAssertEqual(traceText(latestQuestion), "What is the retry delay?")
    captureWindow("Diagnostics latest answer request", in: app)
    app.buttons["diagnostic-view-latest-answer-trace"].click()
    let trace = app.staticTexts["trace-inspector-id"]
    XCTAssertTrue(trace.waitForExistence(timeout: 5))
    let expectedTrace = traceText(trace)
    XCTAssertEqual(expectedTrace.count, 32)
    assertTraceText(
      "What is the retry delay?", identifier: "trace-model-question", in: app)
    assertTraceText(
      "The retry cooldown is 47 seconds.", identifier: "trace-model-source-1-part-0", in: app)
    captureWindow("Diagnostics trace question and source", in: app)
    app.buttons["trace-inspector-close"].click()
    XCTAssertTrue(trace.waitForNonExistence(timeout: 5))
    return expectedTrace
  }

  private func narrowWindow(_ window: XCUIElement) {
    let compactWidth: CGFloat = 900
    let delta = compactWidth - window.frame.width
    if delta < -10 {
      let corner = window.coordinate(withNormalizedOffset: CGVector(dx: 1, dy: 1))
        .withOffset(CGVector(dx: -2, dy: -2))
      corner.click(forDuration: 0.2, thenDragTo: corner.withOffset(CGVector(dx: delta, dy: 0)))
    }
    XCTAssertEqual(window.frame.width, compactWidth, accuracy: 10)
  }

  func testAnswersCancelsAnInFlightGenerationAndAcceptsTheNextQuestion() throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    let assets = try launchAnswerFixture(in: app, directory: directory)
    let original = try BackendEndpoint.read(in: directory)
    try seedAnswerCorpus(in: directory)
    app.selectWorkspace("answers")
    enterLocalQuestion("Hold the answer about the retry delay", in: app)

    let started = assets.appending(path: "Qwen3-4B-Instruct-2507-Q4_K_M.in-flight")
    let generating = XCTNSPredicateExpectation(
      predicate: NSPredicate { _, _ in
        FileManager.default.fileExists(atPath: started.path)
      }, object: nil)
    let result = XCTWaiter.wait(for: [generating], timeout: 8)
    if result != .completed {
      XCTFail("The fixture generation did not start. \(workerDiagnostics(in: assets, app: app))")
      return
    }
    let cancel = app.buttons["Cancel answer"]
    let progress = app.staticTexts["answer-generation-progress"]
    let picker = app.descendants(matching: .any).matching(identifier: "generation-model-picker")
      .firstMatch
    let citation = app.descendants(matching: .any).matching(identifier: "answer-citation")
      .firstMatch
    XCTAssertTrue(cancel.isEnabled)
    XCTAssertTrue(progress.exists)
    verifyBusyModelMenu(picker, in: app)
    cancel.click()
    let message = app.staticTexts["answer-feedback"]
    XCTAssertTrue(message.waitForExistence(timeout: 5))
    XCTAssertEqual(message.value as? String ?? message.label, "The answer request was cancelled.")
    XCTAssertTrue(cancel.waitForNonExistence(timeout: 5))
    XCTAssertFalse(progress.exists)
    XCTAssertFalse(citation.exists)
    XCTAssertTrue(picker.isEnabled)
    XCTAssertTrue(app.buttons["Ask locally"].isEnabled)
    captureWindow("Answers cancelled", in: app)
    let cancelledTrace = try inspectAnswerTrace(outcome: "Cancelled", in: app)

    enterLocalQuestion("What is the retry delay?", in: app)
    verifyCitedAnswer(in: app)
    XCTAssertFalse(message.exists)
    XCTAssertFalse(progress.exists)
    captureWindow("Answers", in: app)
    XCTAssertNotEqual(try inspectAnswerTrace(outcome: "Succeeded", in: app), cancelledTrace)
    inspectExhaustedContext(in: app)
    try assertSameSession(as: original, in: directory)
  }

  func testAnswerHistoryPersistsACitedAnswerAndClearsItFromSettings() async throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchAnswerFixture(in: app, directory: directory, hybrid: true)
    try seedAnswerCorpus(in: directory)
    let historySize = verifyDisabledHistory(in: app)
    let endpoint = try BackendEndpoint.read(in: directory)
    try await enableAnswerHistory(in: app, endpoint: endpoint)
    try await enableHybridIndex(in: endpoint)

    let question = "What is the retry delay?"
    app.selectWorkspace("answers")
    enterLocalQuestion(question, in: app)
    verifyCitedAnswer(in: app)
    let originalTrace = try inspectAnswerTrace(outcome: "Succeeded", hybrid: true, in: app)
    let previous = try BackendEndpoint.read(in: directory)
    try await assertHistoryContains(question, in: previous)
    app.terminate()
    app.launch()
    let ready = app.descendants(matching: .any).matching(identifier: "backend-ready").firstMatch
    XCTAssertTrue(ready.waitForExistence(timeout: 12))
    let current = try BackendEndpoint.read(in: directory)
    XCTAssertNotEqual(current.processId, previous.processId)
    try await assertHistoryRetention(12, in: current)
    try await assertHistoryContains(question, in: current)
    try verifySavedAnswer(question, traceID: originalTrace, sheetSize: historySize, in: app)

    app.selectWorkspace("settings")
    app.buttons["clear-answer-history"].click()
    let confirm = app.sheets.buttons["Clear History"]
    XCTAssertTrue(confirm.waitForExistence(timeout: 5))
    captureWindow("Clear history confirmation", in: app)
    confirm.click()
    XCTAssertTrue(app.staticTexts["Answer history cleared."].waitForExistence(timeout: 5))
    app.selectWorkspace("answers")
    app.buttons["answer-history"].click()
    XCTAssertTrue(app.staticTexts["No saved answers."].waitForExistence(timeout: 5))
    XCTAssertFalse(app.staticTexts["saved-answer-question"].exists)
    XCTAssertFalse(app.buttons["saved-answer-1"].exists)
  }

  func testAnswerLanguageSearchSelectsAndSavesJapanese() {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = directory.path
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    app.launch()
    let ready = app.descendants(matching: .any).matching(identifier: "backend-ready").firstMatch
    XCTAssertTrue(ready.waitForExistence(timeout: 12))
    app.selectWorkspace("settings")

    let picker = app.buttons["answer-language-picker"]
    XCTAssertTrue(picker.waitForExistence(timeout: 5))
    XCTAssertEqual(picker.value as? String, "Automatic")
    picker.click()

    let search = app.textFields["answer-language-search"]
    XCTAssertTrue(search.waitForExistence(timeout: 5))
    XCTAssertTrue(app.buttons["answer-language-en"].exists)
    captureWindow("Answer Language picker", in: app)
    search.click()
    search.typeText("ja")
    let japanese = app.buttons["answer-language-ja"]
    XCTAssertTrue(japanese.waitForExistence(timeout: 5))
    XCTAssertFalse(app.buttons["answer-language-en"].exists)
    XCTAssertTrue(japanese.label.contains("日本語"))
    japanese.click()

    app.buttons["Save"].click()
    XCTAssertTrue(app.staticTexts["Settings saved."].waitForExistence(timeout: 5))
    XCTAssertEqual(picker.value as? String, "日本語")
  }

  private func verifyDisabledHistory(in app: XCUIApplication) -> CGSize {
    app.selectWorkspace("answers")
    app.buttons["answer-history"].click()
    let sheet = app.sheets.firstMatch
    XCTAssertTrue(sheet.waitForExistence(timeout: 5))
    let historySize = sheet.frame.size
    let settings = app.buttons["open-answer-history-settings"]
    XCTAssertTrue(settings.waitForExistence(timeout: 5))
    XCTAssertEqual(settings.frame.midX, sheet.frame.midX, accuracy: 2)
    XCTAssertEqual(sheet.frame.size, historySize)
    captureWindow("Answer history saving off", in: app)
    app.buttons["Done"].click()
    return historySize
  }

  private func assertHistoryContains(_ question: String, in endpoint: BackendEndpoint)
    async throws
  {
    struct Page: Decodable {
      struct Entry: Decodable {
        let question: String
      }
      let entries: [Entry]
    }
    var request = URLRequest(url: endpoint.url.appending(path: "v1/answer-history"))
    request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let (body, response) = try await URLSession.shared.data(for: request)
    XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
    XCTAssertEqual(
      try JSONDecoder().decode(Page.self, from: body).entries.map(\.question), [question])
  }

  private func setEmbeddingIndex(_ enabled: Bool, in endpoint: BackendEndpoint) async throws {
    var request = URLRequest(url: endpoint.url.appending(path: "v1/retrieval/index"))
    request.httpMethod = "PUT"
    request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.httpBody = try JSONEncoder().encode(["enabled": enabled])
    let (_, response) = try await URLSession.shared.data(for: request)
    XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
  }

  private func enableHybridIndex(in endpoint: BackendEndpoint) async throws {
    struct Progress: Decodable {
      let state: String
      let indexed: Int
    }
    try await setEmbeddingIndex(true, in: endpoint)
    var progress = URLRequest(url: endpoint.url.appending(path: "v1/retrieval"))
    progress.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let deadline = ContinuousClock.now + .seconds(8)
    repeat {
      let (body, response) = try await URLSession.shared.data(for: progress)
      XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
      let state = try JSONDecoder().decode(Progress.self, from: body)
      if state.state == "ready", state.indexed == 1 { return }
      try await Task.sleep(for: .milliseconds(100))
    } while ContinuousClock.now < deadline
    XCTFail("The fixture passage did not reach the semantic index.")
  }

  private func enableAnswerHistory(in app: XCUIApplication, endpoint: BackendEndpoint) async throws
  {
    app.selectWorkspace("settings")
    let toggle = app.descendants(matching: .any)
      .matching(identifier: "answer-history-enabled").firstMatch
    XCTAssertTrue(toggle.waitForExistence(timeout: 5))
    XCTAssertEqual(toggle.value as? Int, 0)
    toggle.click()
    XCTAssertEqual(toggle.value as? Int, 1)
    let clear = app.buttons["clear-answer-history"]
    let actionRowY = clear.frame.midY
    chooseRetention("7 days", in: app)
    try await assertHistoryRetention(7, in: endpoint)
    XCTAssertFalse(app.buttons["Save History Settings"].exists)
    XCTAssertEqual(clear.frame.midY, actionRowY, accuracy: 1)
    captureWindow("Answer history settings", in: app)

    chooseRetention("Custom…", in: app)
    let days = app.textFields["answer-history-custom-days"]
    XCTAssertTrue(days.waitForExistence(timeout: 5))
    days.click()
    days.typeKey("a", modifierFlags: .command)
    days.typeText("12")
    days.typeKey(.tab, modifierFlags: [])
    captureWindow("Answer history custom retention", in: app)
    try await assertHistoryRetention(12, in: endpoint)
  }

  private func chooseRetention(_ title: String, in app: XCUIApplication) {
    let picker = app.descendants(matching: .any)
      .matching(identifier: "answer-history-retention").firstMatch
    XCTAssertTrue(picker.waitForExistence(timeout: 5))
    picker.click()
    let option = app.menuItems[title]
    XCTAssertTrue(option.waitForExistence(timeout: 5))
    option.click()
  }

  private func assertHistoryRetention(_ days: Int, in endpoint: BackendEndpoint) async throws {
    struct Settings: Decodable {
      let enabled: Bool
      let retentionDays: Int
    }
    var request = URLRequest(url: endpoint.url.appending(path: "v1/answer-history/settings"))
    request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let deadline = ContinuousClock.now + .seconds(5)
    repeat {
      let (body, response) = try await URLSession.shared.data(for: request)
      XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
      let settings = try JSONDecoder().decode(Settings.self, from: body)
      if settings.enabled, settings.retentionDays == days { return }
      try await Task.sleep(for: .milliseconds(100))
    } while ContinuousClock.now < deadline
    XCTFail("Enabled history did not persist a \(days)-day retention.")
  }

  private func verifySavedAnswer(
    _ question: String, traceID: String, sheetSize: CGSize, in app: XCUIApplication
  ) throws {
    app.selectWorkspace("answers")
    app.buttons["answer-history"].click()
    let saved = app.buttons["saved-answer-1"]
    XCTAssertTrue(saved.waitForExistence(timeout: 5))
    XCTAssertEqual(app.sheets.firstMatch.frame.size, sheetSize)
    saved.click()
    let savedQuestion = app.staticTexts["saved-answer-question"]
    let detail = try XCTUnwrap(savedQuestion.waitForExistence(timeout: 5) ? savedQuestion : nil)
    XCTAssertEqual(detail.value as? String ?? detail.label, question)
    verifyCitedAnswer(in: app.sheets.firstMatch)
    XCTAssertEqual(app.sheets.firstMatch.frame.size, sheetSize)
    captureWindow("Answer history detail", in: app)
    app.buttons["view-answer-trace"].click()
    let trace = app.staticTexts["trace-inspector-id"]
    XCTAssertTrue(trace.waitForExistence(timeout: 5))
    XCTAssertEqual(trace.value as? String ?? trace.label, traceID)
    let traceQuestion = app.staticTexts["trace-saved-answer-question"]
    XCTAssertFalse(traceQuestion.exists)
    let savedAnswer = app.radioGroups["trace-inspector-pane"].radioButtons["Answer"]
    XCTAssertTrue(savedAnswer.waitForExistence(timeout: 5))
    savedAnswer.click()
    XCTAssertTrue(traceQuestion.waitForExistence(timeout: 5))
    XCTAssertEqual(traceQuestion.value as? String ?? traceQuestion.label, question)
    verifyCitedAnswer(in: app.sheets.firstMatch)
    captureWindow("Saved answer in trace", in: app)
    app.buttons["trace-inspector-close"].click()
    XCTAssertTrue(trace.waitForNonExistence(timeout: 5))
  }

  private func verifyBusyModelMenu(_ picker: XCUIElement, in app: XCUIApplication) {
    XCTAssertTrue(picker.isEnabled)
    picker.click()
    let profile = app.menuItems["Qwen3 4B · Q5_K_M"]
    let manage = app.menuItems["Manage Models…"]
    XCTAssertTrue(profile.waitForExistence(timeout: 5))
    XCTAssertFalse(profile.isEnabled)
    XCTAssertTrue(manage.exists)
    XCTAssertTrue(manage.isEnabled)
    app.typeKey(.escape, modifierFlags: [])
  }

  private func submitQuestionFromKeyboard(in app: XCUIApplication) {
    let question = app.textViews["answer-question"]
    XCTAssertTrue(question.waitForExistence(timeout: 5))
    question.click()
    let result = app.staticTexts["answer-result-text"]
    XCTAssertFalse(result.exists)
    app.typeKey(.return, modifierFlags: .command)
    XCTAssertTrue(result.waitForExistence(timeout: 5))
    XCTAssertEqual(
      result.value as? String ?? result.label,
      "The search returned zero candidates. Answer generation was skipped.")
    captureWindow("Answers insufficient evidence", in: app)
  }

  private func enterTwoQuestionLines(in question: XCUIElement) {
    question.click()
    question.typeText("First line")
    question.typeKey(.return, modifierFlags: .shift)
    question.typeText("More detail")
    XCTAssertEqual(question.value as? String, "First line\nMore detail")
  }

  private func verifyAlternateProfile(
    in endpoint: BackendEndpoint, app: XCUIApplication, assets: URL
  )
    async throws
  {
    var request = URLRequest(url: endpoint.url.appending(path: "v1/generation/models"))
    request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let deadline = ContinuousClock.now + .seconds(5)
    while ContinuousClock.now < deadline {
      let (body, response) = try await URLSession.shared.data(for: request)
      XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
      let models = try JSONDecoder().decode(ModelCatalog.self, from: body)
      if models.activeProfile == "qwen3-4b-instruct-2507-q5-k-m-5bde5e9d-v1" { return }
      try await Task.sleep(for: .milliseconds(100))
    }
    XCTFail("The backend did not activate Q5_K_M. \(workerDiagnostics(in: assets, app: app))")
  }

  private func verifyCitedAnswer(in view: XCUIElement) {
    let citation = view.descendants(matching: .any).matching(identifier: "answer-citation")
      .firstMatch
    XCTAssertTrue(citation.waitForExistence(timeout: 8), view.debugDescription)
    let citationText = citation.label.isEmpty ? citation.value as? String : citation.label
    XCTAssertEqual(citationText, "src/outbound.rs @ r-native", view.debugDescription)
    let quote = view.staticTexts["answer-citation-quote-0"]
    XCTAssertTrue(quote.exists)
    XCTAssertEqual(quote.value as? String ?? quote.label, "The retry cooldown is 47 seconds.")
    XCTAssertTrue(view.staticTexts["The configured wait is 47 seconds."].exists)
  }

  private func inspectAnswerTrace(outcome: String, hybrid: Bool = false, in app: XCUIApplication)
    throws -> String
  {
    let viewTrace = app.buttons["view-answer-trace"]
    XCTAssertTrue(viewTrace.waitForExistence(timeout: 5))
    viewTrace.click()
    if outcome == "Succeeded" {
      let question = app.staticTexts["trace-model-question"]
      XCTAssertTrue(question.waitForExistence(timeout: 5))
      XCTAssertEqual(traceText(question), "What is the retry delay?")
      assertTraceText(
        "The retry cooldown is 47 seconds.",
        identifier: "trace-model-source-1-part-0", in: app)
      verifyCitedAnswer(in: app.sheets.firstMatch)
      captureWindow("Answer question sources and result", in: app)
      app.radioGroups["trace-inspector-pane"].radioButtons["Spans"].click()
    }
    let step = app.staticTexts["trace-step-api_answer"]
    XCTAssertTrue(step.waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["trace-step-answer_retrieval"].exists)
    XCTAssertTrue(app.staticTexts["trace-step-answer_generation"].exists)
    let elapsed = app.staticTexts["trace-duration-api_answer"]
    XCTAssertTrue(isElapsedTime(elapsed.value as? String ?? elapsed.label))
    assertTraceText("POST /v1/answers", identifier: "trace-span-operation", in: app)
    assertTraceText("POST /v1/answers", identifier: "trace-span-http-request", in: app)
    assertTraceText(
      outcome == "Cancelled" ? "409" : "200", identifier: "trace-span-http-status", in: app)
    if outcome == "Cancelled" {
      assertTraceText("answer_cancelled", identifier: "trace-span-problem-code", in: app)
    }
    XCTAssertTrue(app.staticTexts[outcome].firstMatch.exists)
    XCTAssertTrue(app.staticTexts["trace-log-api_answer"].firstMatch.exists)
    let trace = app.staticTexts["trace-inspector-id"]
    let id = try XCTUnwrap(trace.value as? String ?? trace.label)
    XCTAssertEqual(id.count, 32)
    inspectAnswerSpan(outcome: outcome, in: app)
    if hybrid {
      app.staticTexts["trace-step-answer_retrieval"].click()
      XCTAssertTrue(app.staticTexts["Hybrid Fusion"].waitForExistence(timeout: 5))
      XCTAssertTrue(app.staticTexts["Lexical #1"].exists)
      XCTAssertTrue(app.staticTexts["Dense #1"].exists)
      XCTAssertTrue(app.staticTexts["Fusion #1"].exists)
    }
    captureWindow("Answer trace \(outcome)", in: app)
    app.buttons["trace-inspector-close"].click()
    XCTAssertTrue(trace.waitForNonExistence(timeout: 5))
    return id
  }

  private func inspectAnswerSpan(outcome: String, in app: XCUIApplication) {
    app.staticTexts["trace-step-answer_generation"].click()
    assertTraceText("inference.generate", identifier: "trace-span-operation", in: app)
    assertTraceText("1 of 1 passages selected", identifier: "trace-context-admitted", in: app)
    if outcome == "Cancelled" {
      assertTraceText("Generation cancelled", identifier: "trace-span-event-0", in: app)
      captureWindow("Cancelled answer context budget", in: app)
    } else {
      captureWindow("Answer context budget", in: app)
      app.staticTexts["trace-step-answer_validation"].click()
      assertTraceText("answer.verify_citations", identifier: "trace-span-operation", in: app)
      assertTraceText("Citations verified", identifier: "trace-span-event-0", in: app)
    }
  }

  private func inspectExhaustedContext(in app: XCUIApplication) {
    enterLocalQuestion("Budget none: What is the retry delay?", in: app)
    let viewTrace = app.buttons["view-answer-trace"]
    XCTAssertTrue(viewTrace.waitForExistence(timeout: 8))
    viewTrace.click()
    app.radioGroups["trace-inspector-pane"].radioButtons["Spans"].click()
    let generation = app.staticTexts["trace-step-answer_generation"]
    XCTAssertTrue(generation.waitForExistence(timeout: 5))
    generation.click()
    assertTraceText("0 of 1 passages selected", identifier: "trace-context-admitted", in: app)
    assertTraceText(
      "No evidence fit within the \(3455.formatted())-token limit.",
      identifier: "trace-context-no-fit", in: app)
    captureWindow("Exhausted answer context budget", in: app)
    app.buttons["trace-inspector-close"].click()
  }

  private func assertSameSession(as original: BackendEndpoint, in directory: URL) throws {
    let current = try BackendEndpoint.read(in: directory)
    XCTAssertEqual(current.processId, original.processId)
    XCTAssertEqual(current.url, original.url)
    XCTAssertTrue(current.token == original.token, "The session credential changed.")
  }

  private func launchAnswerFixture(in app: XCUIApplication, directory: URL, hybrid: Bool = false)
    throws -> URL
  {
    let assets = try stageAnswerWorker(in: directory, app: app)
    try Data().write(to: assets.appending(path: "Qwen3-4B-Instruct-2507-Q5_K_M.gguf"))
    if hybrid {
      try Data("formulation fixture".utf8).write(
        to: assets.appending(path: "Qwen3-Embedding-0.6B-Q8_0.gguf"))
    }
    app.launch()
    let ready = app.descendants(matching: .any).matching(identifier: "backend-ready").firstMatch
    XCTAssertTrue(ready.waitForExistence(timeout: 12))
    return assets
  }

  private func workerDiagnostics(in assets: URL, app: XCUIApplication) -> String {
    let files = (try? FileManager.default.contentsOfDirectory(atPath: assets.path)) ?? []
    let feedback = app.staticTexts["answer-feedback"].value as? String
    return "Fixture files \(files.sorted()), app feedback \(feedback ?? "none")."
  }

  private func seedAnswerCorpus(in directory: URL) throws {
    let sqlite = Process()
    sqlite.executableURL = URL(filePath: "/usr/bin/sqlite3")
    sqlite.arguments = [
      "-bail", "-cmd", ".timeout 2000", directory.appending(path: "context.sqlite3").path,
      """
      BEGIN;
      INSERT INTO collections(name) VALUES ('native-answer-fixture');
      INSERT INTO documents(collection, source, revision, kind, title, content)
      VALUES ('native-answer-fixture', 'src/outbound.rs', 'r-native', 'history',
              'Retry cooldown', 'The retry cooldown is 47 seconds.');
      COMMIT;
      """,
    ]
    try sqlite.run()
    sqlite.waitUntilExit()
    XCTAssertEqual(sqlite.terminationStatus, 0)
  }

  private func enterLocalQuestion(_ text: String, in app: XCUIApplication) {
    let question = app.textViews["answer-question"]
    XCTAssertTrue(question.exists)
    question.click()
    question.typeKey("a", modifierFlags: .command)
    question.typeText(text)
    XCTAssertEqual(question.value as? String, text)
    question.typeKey(.return, modifierFlags: [])
  }
}
