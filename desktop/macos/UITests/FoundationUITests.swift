import Foundation
import XCTest

@MainActor
final class FoundationUITests: XCTestCase {
  private struct Endpoint: Decodable {
    let url: URL
    let processId: Int32
    let token: String
  }

  private struct ModelCatalog: Decodable {
    let activeProfile: String?
  }

  func testPackagedBackendBecomesReady() {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = directory.path
    app.launchEnvironment["CTXFAB_INFERENCE_DIRECTORY"] = directory.appending(path: "missing").path
    if ProcessInfo.processInfo.environment["CTXFAB_UI_PROBE_MISSING_BACKEND"] == "1" {
      app.launchEnvironment["CTXFAB_TEST_BACKEND_DISABLED"] = "1"
    }
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    app.launch()
    let ready = app.descendants(matching: .any).matching(identifier: "backend-ready").firstMatch
    if !ready.waitForExistence(timeout: 12) {
      let attachment = XCTAttachment(string: app.debugDescription)
      attachment.name = "Backend startup accessibility state"
      attachment.lifetime = .keepAlways
      add(attachment)
      XCTFail("Expected the bundled backend to become ready.")
      return
    }
    XCTAssertTrue(
      app.descendants(matching: .any).matching(identifier: "collected-documents").firstMatch.exists)
    XCTAssertTrue(
      app.descendants(matching: .any).matching(identifier: "searchable-documents").firstMatch.exists
    )
    captureWindow("Overview", in: app)
    app.buttons["Ask a question"].click()
    let question = app.textViews["answer-question"]
    let ask = app.buttons["Ask locally"]
    XCTAssertTrue(question.waitForExistence(timeout: 5))
    XCTAssertTrue(ask.exists)
    XCTAssertGreaterThan(ask.frame.minY, question.frame.minY)
    enterTwoQuestionLines(in: question)
    XCTAssertFalse(ask.isEnabled)
    captureWindow("Answers model setup", in: app)
    verifyModelLibrary(in: app)
  }

  func testMissingBackendHasVisibleFailure() {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = directory.path
    app.launchEnvironment["CTXFAB_TEST_BACKEND_DISABLED"] = "1"
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    app.launch()
    let unavailable = app.staticTexts["backend-unavailable"]
    XCTAssertTrue(unavailable.waitForExistence(timeout: 8))
    openAnswers(in: app)
    let picker = app.descendants(matching: .any).matching(identifier: "generation-model-picker")
      .firstMatch
    XCTAssertTrue(picker.waitForExistence(timeout: 5))
    picker.click()
    app.menuItems["Manage Models…"].click()
    XCTAssertTrue(app.buttons["Open Model Folder"].waitForExistence(timeout: 5))
    captureWindow("Answers model library without backend", in: app)
  }

  func testAnswersSwitchesModelWithoutRestartingAndShowsInsufficientEvidence() async throws {
    continueAfterFailure = false
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    let assets = try launchAnswerFixture(in: app, directory: directory)
    let warmupGate = assets.appending(path: "Qwen3-4B-Instruct-2507-Q5_K_M.startup-gate")
    try Data().write(to: warmupGate)
    let original = try readEndpoint(in: directory)
    openAnswers(in: app)
    let picker = app.descendants(matching: .any).matching(identifier: "generation-model-picker")
      .firstMatch
    XCTAssertTrue(picker.waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["Ask about your corpus"].exists)
    try verifyInstalledModel(in: app, assets: assets)
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

  private func verifyModelLibrary(in app: XCUIApplication) {
    let manage = app.buttons["manage-models"]
    XCTAssertTrue(manage.waitForExistence(timeout: 5))
    manage.click()
    let profile = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"
    let download = app.buttons["download-model-\(profile)"]
    XCTAssertTrue(download.waitForExistence(timeout: 5))
    XCTAssertTrue(download.isEnabled)
    XCTAssertTrue(app.staticTexts["Qwen3 4B · Q4_K_M"].exists)
    let status = app.staticTexts["model-status-\(profile)"]
    let description = status.value as? String ?? status.label
    XCTAssertTrue(description.hasPrefix("Not installed · "))
    XCTAssertGreaterThan(description.count, "Not installed · ".count)
    XCTAssertTrue(app.buttons["Open Model Folder"].exists)
    XCTAssertTrue(app.buttons["Restart Backend"].exists)
    captureWindow("Answers model library", in: app)
    app.buttons["Done"].click()
    XCTAssertEqual(app.textViews["answer-question"].value as? String, "First line\nMore detail")
  }

  private func submitQuestionFromKeyboard(in app: XCUIApplication) {
    let question = app.textViews["answer-question"]
    XCTAssertTrue(question.waitForExistence(timeout: 5))
    question.click()
    let result = app.staticTexts["The local corpus has insufficient evidence."]
    XCTAssertFalse(result.exists)
    app.typeKey(.return, modifierFlags: .command)
    XCTAssertTrue(result.waitForExistence(timeout: 5))
  }

  private func enterTwoQuestionLines(in question: XCUIElement) {
    question.click()
    question.typeText("First line")
    question.typeKey(.return, modifierFlags: [])
    question.typeText("More detail")
    XCTAssertEqual(question.value as? String, "First line\nMore detail")
  }

  private func verifyInstalledModel(in app: XCUIApplication, assets: URL) throws {
    let model = assets.appending(path: "Qwen3-Embedding-0.6B-Q8_0.gguf")
    try Data().write(to: model)
    let file = try FileHandle(forWritingTo: model)
    defer { try? file.close() }
    // The download catalog recognizes installed weights by their pinned byte count.
    try file.truncate(atOffset: 639_150_592)
    let picker = app.descendants(matching: .any).matching(identifier: "generation-model-picker")
      .firstMatch
    XCTAssertTrue(picker.waitForExistence(timeout: 5))
    picker.click()
    app.menuItems["Manage Models…"].click()
    let profile = "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1"
    let status = app.staticTexts["model-status-\(profile)"]
    XCTAssertTrue(status.waitForExistence(timeout: 5))
    XCTAssertTrue((status.value as? String ?? status.label).hasPrefix("Installed · "))
    XCTAssertFalse(app.buttons["download-model-\(profile)"].exists)
    app.buttons["Done"].click()
  }

  private func verifyAlternateProfile(in endpoint: Endpoint, app: XCUIApplication, assets: URL)
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

  func testAnswersCancelsAnInFlightGenerationAndAcceptsTheNextQuestion() throws {
    continueAfterFailure = false
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    let assets = try launchAnswerFixture(in: app, directory: directory)
    let original = try readEndpoint(in: directory)
    try seedAnswerCorpus(in: directory)
    openAnswers(in: app)
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
    let cancelledTrace = try inspectAnswerTrace(outcome: "Failed", in: app)

    enterLocalQuestion("What is the retry delay?", in: app)
    verifyCitedAnswer(citation, in: app)
    XCTAssertFalse(message.exists)
    XCTAssertFalse(progress.exists)
    captureWindow("Answers", in: app)
    XCTAssertNotEqual(try inspectAnswerTrace(outcome: "Succeeded", in: app), cancelledTrace)
    try assertSameSession(as: original, in: directory)
  }

  private func verifyCitedAnswer(_ citation: XCUIElement, in app: XCUIApplication) {
    XCTAssertTrue(citation.waitForExistence(timeout: 8), app.debugDescription)
    let citationText = citation.label.isEmpty ? citation.value as? String : citation.label
    XCTAssertEqual(citationText, "src/outbound.rs @ r-native", app.debugDescription)
    XCTAssertTrue(app.staticTexts["The retry cooldown is 47 seconds."].exists)
  }

  private func inspectAnswerTrace(outcome: String, in app: XCUIApplication) throws -> String {
    let viewTrace = app.buttons["view-answer-trace"]
    XCTAssertTrue(viewTrace.waitForExistence(timeout: 5))
    viewTrace.click()
    let step = app.staticTexts["trace-step-api_answer"]
    XCTAssertTrue(step.waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["trace-step-answer_retrieval"].exists)
    XCTAssertTrue(app.staticTexts["trace-step-answer_generation"].exists)
    let elapsed = app.staticTexts["trace-duration-api_answer"]
    XCTAssertTrue(isElapsedTime(elapsed.value as? String ?? elapsed.label))
    assertTraceText("POST /v1/answers", identifier: "trace-span-operation", in: app)
    XCTAssertTrue(app.staticTexts[outcome].firstMatch.exists)
    XCTAssertTrue(app.staticTexts["trace-log-api_answer"].firstMatch.exists)
    let trace = app.staticTexts["trace-inspector-id"]
    let id = try XCTUnwrap(trace.value as? String ?? trace.label)
    XCTAssertEqual(id.count, 32)
    inspectAnswerSpan(outcome: outcome, in: app)
    captureWindow("Answer trace \(outcome)", in: app)
    app.buttons["Done"].click()
    XCTAssertTrue(trace.waitForNonExistence(timeout: 5))
    return id
  }

  private func inspectAnswerSpan(outcome: String, in app: XCUIApplication) {
    if outcome == "Failed" {
      app.staticTexts["trace-step-answer_generation"].click()
      assertTraceText("inference.generate", identifier: "trace-span-operation", in: app)
      assertTraceText("Generation cancelled", identifier: "trace-span-event-0", in: app)
    } else {
      app.staticTexts["trace-step-answer_validation"].click()
      assertTraceText("answer.verify_citations", identifier: "trace-span-operation", in: app)
      assertTraceText("Citations verified", identifier: "trace-span-event-0", in: app)
    }
  }

  private func assertSameSession(as original: Endpoint, in directory: URL) throws {
    let current = try readEndpoint(in: directory)
    XCTAssertEqual(current.processId, original.processId)
    XCTAssertEqual(current.url, original.url)
    XCTAssertTrue(current.token == original.token, "The session credential changed.")
  }

  private func launchAnswerFixture(in app: XCUIApplication, directory: URL) throws -> URL {
    let assets = directory.appending(path: "inference")
    try FileManager.default.createDirectory(
      at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    try FileManager.default.createDirectory(at: assets, withIntermediateDirectories: false)
    let worker = try XCTUnwrap(
      Bundle(for: FoundationUITests.self).url(forResource: "llama-server", withExtension: nil))
    try FileManager.default.createSymbolicLink(
      at: assets.appending(path: "llama-server"), withDestinationURL: worker)
    try Data().write(to: assets.appending(path: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"))
    try Data().write(to: assets.appending(path: "Qwen3-4B-Instruct-2507-Q5_K_M.gguf"))
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = directory.path
    app.launchEnvironment["CTXFAB_INFERENCE_DIRECTORY"] = assets.path
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

  private func openAnswers(in app: XCUIApplication) {
    navigate(to: "answers", in: app)
  }

  private func navigate(to destination: String, in app: XCUIApplication) {
    app.selectWorkspace(destination)
  }

  private func captureWindow(_ name: String, in app: XCUIApplication) {
    guard ProcessInfo.processInfo.environment["CTXFAB_CAPTURE_UI"] == "1" else { return }
    let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
  }

  private func enterLocalQuestion(_ text: String, in app: XCUIApplication) {
    let question = app.textViews["answer-question"]
    XCTAssertTrue(question.exists)
    question.click()
    question.typeKey("a", modifierFlags: .command)
    question.typeText(text)
    XCTAssertEqual(question.value as? String, text)
    app.buttons["Ask locally"].click()
  }

  func testCrashIsVisibleAndRestartRotatesTheSession() throws {
    continueAfterFailure = false
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
    let original = try readEndpoint(in: directory)
    let termination = kill(original.processId, SIGKILL)
    XCTAssertEqual(termination, 0)
    XCTAssertTrue(app.staticTexts["backend-unavailable"].waitForExistence(timeout: 8))
    XCTAssertTrue(
      app.staticTexts["The local backend exited. Check that its data directory is available."]
        .exists)

    app.buttons["Restart backend"].click()
    XCTAssertTrue(ready.waitForExistence(timeout: 12))
    let restarted = try readEndpoint(in: directory)
    XCTAssertNotEqual(original.processId, restarted.processId)
    XCTAssertTrue(original.token != restarted.token, "Restart must rotate local authorization.")
  }

  func testClosingWindowKeepsTheSessionUntilAppQuit() async throws {
    continueAfterFailure = false
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
    let original = try readEndpoint(in: directory)

    app.typeKey("w", modifierFlags: .command)
    XCTAssertTrue(app.windows.firstMatch.waitForNonExistence(timeout: 5))
    XCTAssertEqual(app.state, .runningForeground)
    var request = URLRequest(url: original.url.appending(path: "/v1/status"))
    request.setValue("Bearer \(original.token)", forHTTPHeaderField: "Authorization")
    let (_, response) = try await URLSession.shared.data(for: request)
    XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
    let afterClose = try readEndpoint(in: directory)
    XCTAssertEqual(afterClose.processId, original.processId)

    app.menuBars.menuBarItems["Context Fabricator"].menuItems["Quit Context Fabricator"].click()
    XCTAssertTrue(app.wait(for: .notRunning, timeout: 8))
    let endpoint = directory.appending(path: "endpoint.json")
    let deadline = ContinuousClock.now + .seconds(5)
    while ContinuousClock.now < deadline,
      FileManager.default.fileExists(atPath: endpoint.path) || kill(original.processId, 0) == 0
    {
      try await Task.sleep(for: .milliseconds(50))
    }
    XCTAssertFalse(FileManager.default.fileExists(atPath: endpoint.path))
    XCTAssertNotEqual(kill(original.processId, 0), 0)
  }

  func testSettingsValidationAndDiagnosticsUseTheBundledBackend() async throws {
    continueAfterFailure = false
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

    navigate(to: "settings", in: app)
    let retention = app.textFields["diagnostic-retention"]
    XCTAssertTrue(retention.waitForExistence(timeout: 5), app.debugDescription)
    let budget = app.textFields["diagnostic-storage-limit"]
    XCTAssertTrue(budget.exists)
    XCTAssertEqual(budget.value as? String, "10")

    enterNumber("0", in: retention)
    app.buttons["Save"].click()
    let rejection = app.staticTexts["Diagnostic retention must be between 1 and 168 hours."]
    XCTAssertTrue(rejection.waitForExistence(timeout: 5))

    enterNumber("48", in: retention)
    enterNumber("1000", in: budget)
    app.buttons["Save"].click()
    XCTAssertTrue(app.staticTexts["Settings saved."].waitForExistence(timeout: 5))
    captureWindow("Settings", in: app)

    navigate(to: "diagnostics", in: app)
    app.buttons["Refresh"].click()
    XCTAssertTrue(app.staticTexts["Application settings saved."].waitForExistence(timeout: 5))
    captureWindow("Diagnostics", in: app)
    try await verifyEmbeddedSignals(app, in: directory)

  }

  func testDiagnosticLogsOpenTheirCorrelatedTrace() async throws {
    continueAfterFailure = false
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
    let endpoint = try readEndpoint(in: directory)
    let first = try await createSearchLog("first fixture", endpoint: endpoint)
    let second = try await createSearchLog("second fixture", endpoint: endpoint)
    XCTAssertNotEqual(first.traceID, second.traceID)

    navigate(to: "diagnostics", in: app)
    inspectDiagnosticTrace(second, in: app)
    inspectDiagnosticTrace(first, expandSpan: true, in: app)
  }

  private struct SearchLogFixture {
    let id: Int64
    let traceID: String
  }

  private struct TelemetryLogs: Decodable {
    struct Record: Decodable {
      let id: Int64
      let name: String
      let traceId: String?
    }
    let records: [Record]
  }

  private func createSearchLog(_ query: String, endpoint: Endpoint) async throws -> SearchLogFixture
  {
    var search = URLRequest(
      url: endpoint.url.appending(path: "v1/search")
        .appending(queryItems: [.init(name: "query", value: query)]))
    search.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let (_, response) = try await URLSession.shared.data(for: search)
    let result = try XCTUnwrap(response as? HTTPURLResponse)
    XCTAssertEqual(result.statusCode, 200)
    let traceID = try XCTUnwrap(result.value(forHTTPHeaderField: "X-Context-Trace-Id"))

    var read = URLRequest(
      url: endpoint.url.appending(path: "v1/telemetry/logs")
        .appending(queryItems: [.init(name: "query", value: traceID)]))
    read.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let deadline = ContinuousClock.now + .seconds(5)
    while ContinuousClock.now < deadline {
      let (data, response) = try await URLSession.shared.data(for: read)
      XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
      let page = try JSONDecoder().decode(TelemetryLogs.self, from: data)
      if let row = page.records.first(where: { $0.name == "api_query" && $0.traceId == traceID }) {
        return .init(id: row.id, traceID: traceID)
      }
      try await Task.sleep(for: .milliseconds(50))
    }
    return try XCTUnwrap(nil, "The fixture search log was not retained.")
  }

  private func inspectDiagnosticTrace(
    _ fixture: SearchLogFixture, expandSpan: Bool = false, in app: XCUIApplication
  ) {
    let action = app.buttons["view-log-trace-\(fixture.id)"]
    guard action.waitForExistence(timeout: 5) else {
      XCTFail("The correlated log should offer View Trace.")
      return
    }
    action.click()
    let trace = app.staticTexts["trace-inspector-id"]
    XCTAssertTrue(trace.waitForExistence(timeout: 5))
    XCTAssertEqual(trace.value as? String ?? trace.label, fixture.traceID)
    XCTAssertTrue(app.staticTexts["trace-step-api_query"].waitForExistence(timeout: 5))
    assertTraceText("Search request", identifier: "trace-span-title", in: app)
    assertTraceText("GET /v1/search", identifier: "trace-span-operation", in: app)
    let duration = app.staticTexts["trace-duration-api_query"]
    XCTAssertTrue(isElapsedTime(traceText(duration)))
    XCTAssertTrue(app.staticTexts["trace-log-api_query"].exists)
    if expandSpan { inspectSearchChild(in: app) }
    captureWindow("Diagnostics log trace", in: app)
    if expandSpan { inspectTraceWideLogs(in: app) }
    app.buttons["Done"].click()
    XCTAssertTrue(trace.waitForNonExistence(timeout: 5))
  }

  private func inspectSearchChild(in app: XCUIApplication) {
    assertTraceText("BM25 lookup selected", identifier: "trace-span-event-0", in: app)
    let eventOffset = app.staticTexts["trace-span-event-offset-0"]
    let offsetText = traceText(eventOffset)
    XCTAssertTrue(offsetText.hasPrefix("+") && isElapsedTime(offsetText))
    let id = app.staticTexts["trace-span-id"]
    let rootID = traceText(id)
    XCTAssertEqual(rootID.count, 16)
    let child = app.staticTexts["trace-step-lexical_lookup"]
    XCTAssertTrue(child.exists)
    child.click()
    assertTraceText("Look up keywords", identifier: "trace-span-title", in: app)
    assertTraceText("search.bm25", identifier: "trace-span-operation", in: app)
    assertTraceText(rootID, identifier: "trace-span-parent", in: app)
    XCTAssertTrue(app.staticTexts["trace-log-lexical_lookup"].exists)
    XCTAssertFalse(app.staticTexts["trace-log-api_query"].exists)
  }

  private func inspectTraceWideLogs(in app: XCUIApplication) {
    let logs = app.radioGroups["trace-inspector-pane"].radioButtons["Logs"]
    XCTAssertTrue(logs.waitForExistence(timeout: 5))
    logs.click()
    XCTAssertTrue(app.staticTexts["trace-log-api_query"].waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["trace-log-lexical_lookup"].exists)
    captureWindow("Trace-wide logs", in: app)
  }

  private func assertTraceText(_ value: String, identifier: String, in app: XCUIApplication) {
    let element = app.staticTexts[identifier]
    XCTAssertTrue(element.waitForExistence(timeout: 5))
    XCTAssertEqual(traceText(element), value)
  }

  private func traceText(_ element: XCUIElement) -> String {
    let value = element.value as? String ?? ""
    return value.isEmpty ? element.label : value
  }

  private func isElapsedTime(_ value: String) -> Bool {
    [" µs", " ms", " s"].contains { value.hasSuffix($0) }
  }

  func testLaunchToBackendReadyPerformance() {
    continueAfterFailure = false
    let options = XCTMeasureOptions()
    options.iterationCount = 3
    options.invocationOptions = [.manuallyStart, .manuallyStop]
    measure(metrics: [XCTClockMetric()], options: options) {
      let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
      let app = XCUIApplication()
      app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = directory.path
      defer {
        app.terminate()
        try? FileManager.default.removeItem(at: directory)
      }
      let ready = app.descendants(matching: .any).matching(identifier: "backend-ready").firstMatch
      startMeasuring()
      app.launch()
      let started = ready.waitForExistence(timeout: 12)
      stopMeasuring()
      XCTAssertTrue(started, "The packaged app must reach backend readiness on each launch.")
    }
  }

  private func verifyEmbeddedSignals(_ app: XCUIApplication, in directory: URL) async throws {
    let endpoint = try readEndpoint(in: directory)
    let enable = app.buttons["Debug for 5 minutes"]
    XCTAssertTrue(enable.waitForExistence(timeout: 5))
    enable.click()
    let disable = app.buttons["Disable debug"]
    XCTAssertTrue(disable.waitForExistence(timeout: 5))

    var request = URLRequest(
      url: endpoint.url.appending(path: "v1/search").appending(queryItems: [
        URLQueryItem(name: "query", value: "nativefixture")
      ]))
    request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let (_, response) = try await URLSession.shared.data(for: request)
    XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)

    guard selectDiagnosticTab("Storage", in: app) else { return }
    let retention = app.staticTexts["diagnostics-retention"]
    XCTAssertTrue(retention.waitForExistence(timeout: 5))
    XCTAssertEqual(retention.value as? String ?? retention.label, "48 h")
    let diskBudget = app.staticTexts["diagnostics-budget"]
    XCTAssertTrue(diskBudget.waitForExistence(timeout: 5))
    XCTAssertEqual(
      diskBudget.value as? String ?? diskBudget.label,
      Int64(1_000_000_000).formatted(.byteCount(style: .file)))
    captureWindow("Diagnostics-Storage", in: app)
    XCTAssertTrue(app.staticTexts["Debug expires"].exists)
    app.buttons["Purge expired"].click()
    disable.click()
    XCTAssertTrue(enable.waitForExistence(timeout: 5))

    guard selectDiagnosticTab("Metrics", in: app) else { return }
    XCTAssertFalse(logLevel(in: app).isHittable, "The level picker belongs to Logs.")
    app.buttons["Refresh"].click()
    let metric = app.descendants(matching: .any).matching(identifier: "telemetry-metric").firstMatch
    XCTAssertTrue(metric.waitForExistence(timeout: 5))
    guard selectDiagnosticTab("Traces", in: app) else { return }
    let span = app.descendants(matching: .any).matching(identifier: "telemetry-span").firstMatch
    XCTAssertTrue(span.waitForExistence(timeout: 5))
    captureWindow("Diagnostics-Traces", in: app)
    verifyRecordedSpanOpensTrace(in: app)
    guard selectDiagnosticTab("Logs", in: app) else { return }
    let log = app.descendants(matching: .any)
      .matching(NSPredicate(format: "identifier BEGINSWITH %@", "telemetry-log-")).firstMatch
    XCTAssertTrue(log.waitForExistence(timeout: 5))
    verifyLogFilters(in: app)
  }

  private func verifyRecordedSpanOpensTrace(in app: XCUIApplication) {
    let open = app.buttons.matching(
      NSPredicate(format: "identifier BEGINSWITH %@", "view-span-trace-")
    )
    .firstMatch
    XCTAssertTrue(open.waitForExistence(timeout: 5))
    open.click()
    let operation = app.staticTexts["trace-span-operation"]
    XCTAssertTrue(operation.waitForExistence(timeout: 5))
    XCTAssertNotEqual(operation.value as? String ?? operation.label, "Unavailable")
    app.buttons["Done"].click()
    XCTAssertTrue(operation.waitForNonExistence(timeout: 5))
  }

  private func selectDiagnosticTab(_ title: String, in app: XCUIApplication) -> Bool {
    let tab = app.windows.firstMatch.descendants(matching: .any)
      .matching(NSPredicate(format: "label == %@ OR title == %@", title, title)).firstMatch
    guard tab.waitForExistence(timeout: 5) else {
      XCTFail("Diagnostics tab \(title) could not be found.")
      return false
    }
    tab.click()
    return true
  }

  private func verifyLogFilters(in app: XCUIApplication) {
    let window = app.windows.firstMatch
    let picker = logLevel(in: app)
    let infoLog = logRow("backend_startup", in: window)
    let queryLog = logRow("api_query", in: window)
    let lexicalLog = logRow("lexical_lookup", in: window)
    XCTAssertTrue(picker.waitForExistence(timeout: 5))
    XCTAssertGreaterThan(
      picker.frame.midX, window.frame.midX, "Align the picker to the trailing edge.")
    XCTAssertTrue(infoLog.waitForExistence(timeout: 5), "All Levels must include the Info log.")
    captureWindow("Diagnostics Log Levels", in: app)
    selectLogLevel("Debug", picker: picker, in: app)
    XCTAssertTrue(
      queryLog.waitForExistence(timeout: 5),
      "The debug-filtered logs should include the native search operation.")
    XCTAssertTrue(lexicalLog.exists)
    XCTAssertTrue(infoLog.waitForNonExistence(timeout: 5), "Debug must exclude the Info log.")
    XCTAssertFalse(app.staticTexts["Backend Events"].exists)
    captureWindow("Diagnostics Debug Filter", in: app)

    verifyLogSearch(queryLog: queryLog, lexicalLog: lexicalLog, in: app)
    selectLogLevel("All Levels", picker: picker, in: app)
    XCTAssertTrue(
      infoLog.waitForExistence(timeout: 5), "Resetting the level must restore the Info log.")
    XCTAssertTrue(app.staticTexts["Backend Events"].exists)
  }

  private func verifyLogSearch(
    queryLog: XCUIElement, lexicalLog: XCUIElement, in app: XCUIApplication
  ) {
    let window = app.windows.firstMatch
    let search = window.searchFields.firstMatch
    if !search.exists, window.buttons["Search"].exists { window.buttons["Search"].click() }
    XCTAssertTrue(search.waitForExistence(timeout: 5))
    search.click()
    app.typeText("api")
    XCTAssertTrue(lexicalLog.waitForNonExistence(timeout: 5))
    XCTAssertTrue(queryLog.waitForExistence(timeout: 5), "Search must keep the matching Debug log.")
    clearLogSearch(search)

    app.typeText("absent")
    let noResults = window.descendants(matching: .any)
      .matching(identifier: "diagnostic-logs-no-results").firstMatch
    XCTAssertTrue(noResults.waitForExistence(timeout: 5), "The empty result should be explicit.")
    XCTAssertTrue(queryLog.waitForNonExistence(timeout: 5))
    clearLogSearch(search)
    XCTAssertTrue(queryLog.waitForExistence(timeout: 5))
  }

  private func clearLogSearch(_ search: XCUIElement) {
    search.typeKey("a", modifierFlags: .command)
    search.typeKey(.delete, modifierFlags: [])
  }

  private func selectLogLevel(_ title: String, picker: XCUIElement, in app: XCUIApplication) {
    picker.click()
    app.menuItems[title].click()
    XCTAssertTrue(
      picker.title == title || picker.value as? String == title,
      "The level picker should show \(title).")
  }

  private func logLevel(in app: XCUIApplication) -> XCUIElement {
    app.windows.firstMatch.descendants(matching: .any)
      .matching(identifier: "diagnostic-log-level").firstMatch
  }

  private func logRow(_ name: String, in window: XCUIElement) -> XCUIElement {
    window.descendants(matching: .any).matching(identifier: "telemetry-log-\(name)").firstMatch
  }

  private func enterNumber(_ value: String, in field: XCUIElement) {
    field.click()
    field.typeKey("a", modifierFlags: .command)
    field.typeText(value)
    field.typeKey(XCUIKeyboardKey.tab.rawValue, modifierFlags: [])
  }

  private func readEndpoint(in directory: URL) throws -> Endpoint {
    try JSONDecoder().decode(
      Endpoint.self, from: Data(contentsOf: directory.appending(path: "endpoint.json"))
    )
  }
}
