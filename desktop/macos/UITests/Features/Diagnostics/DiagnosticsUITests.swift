import Foundation
import XCTest

@MainActor
final class DiagnosticsUITests: XCTestCase {
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

    app.selectWorkspace("settings")
    let retention = app.textFields["diagnostic-retention"]
    XCTAssertTrue(retention.waitForExistence(timeout: 5), app.debugDescription)
    let budget = app.textFields["diagnostic-storage-limit"]
    XCTAssertTrue(budget.exists)
    XCTAssertEqual(budget.value as? String, "10")

    enterNumber("0", in: retention)
    app.buttons["Save"].click()
    let rejection = app.staticTexts["Diagnostic retention must be between 1 and 168 hours."]
    XCTAssertTrue(rejection.waitForExistence(timeout: 5))
    captureWindow("Settings invalid retention", in: app)

    enterNumber("48", in: retention)
    enterNumber("1000", in: budget)
    app.buttons["Save"].click()
    XCTAssertTrue(app.staticTexts["Settings saved."].waitForExistence(timeout: 5))
    captureWindow("Settings", in: app)

    app.selectWorkspace("diagnostics")
    guard selectDiagnosticTab("Logs", in: app) else { return }
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
    let endpoint = try BackendEndpoint.read(in: directory)
    let first = try await createSearchLog("first fixture", endpoint: endpoint)
    let second = try await createSearchLog("second fixture", endpoint: endpoint)
    XCTAssertNotEqual(first.traceID, second.traceID)

    app.selectWorkspace("diagnostics")
    guard selectDiagnosticTab("Logs", in: app) else { return }
    inspectDiagnosticTrace(second, in: app)
    inspectDiagnosticTrace(first, expandSpan: true, in: app)
    openTraceByIdentifier(second.traceID, in: app)
  }

  private func openTraceByIdentifier(_ identifier: String, in app: XCUIApplication) {
    app.buttons["open-diagnostic-trace"].click()
    let field = app.textFields["diagnostic-trace-id"]
    let open = app.buttons["diagnostic-open-trace"]
    XCTAssertTrue(field.waitForExistence(timeout: 5))
    XCTAssertFalse(open.isEnabled)
    captureWindow("Diagnostics open trace by ID", in: app)
    field.click()
    for digit in identifier {
      field.typeKey(String(digit), modifierFlags: [])
    }
    XCTAssertEqual(field.value as? String, identifier)
    XCTAssertTrue(open.isEnabled)
    open.click()
    let trace = app.staticTexts["trace-inspector-id"]
    XCTAssertTrue(trace.waitForExistence(timeout: 5))
    XCTAssertEqual(traceText(trace), identifier)
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

  private func createSearchLog(_ query: String, endpoint: BackendEndpoint) async throws
    -> SearchLogFixture
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
    captureWindow(expandSpan ? "Diagnostics child span" : "Diagnostics root span", in: app)
    if expandSpan { inspectTraceWideLogs(in: app) }
    app.buttons["trace-inspector-close"].click()
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

  private func verifyEmbeddedSignals(_ app: XCUIApplication, in directory: URL) async throws {
    let endpoint = try BackendEndpoint.read(in: directory)
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
    let purge = app.buttons["Purge expired"]
    XCTAssertEqual(purge.frame.maxX, diskBudget.frame.maxX, accuracy: 3)
    captureWindow("Diagnostics-Storage", in: app)
    XCTAssertTrue(app.staticTexts["Debug expires"].exists)
    purge.click()
    disable.click()
    XCTAssertTrue(enable.waitForExistence(timeout: 5))

    guard selectDiagnosticTab("Metrics", in: app) else { return }
    XCTAssertFalse(logLevel(in: app).isHittable, "The level picker belongs to Logs.")
    app.buttons["Refresh"].click()
    let metric = app.descendants(matching: .any).matching(identifier: "telemetry-metric").firstMatch
    XCTAssertTrue(metric.waitForExistence(timeout: 5))
    captureWindow("Diagnostics-Metrics", in: app)
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
    app.buttons["trace-inspector-close"].click()
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
}
