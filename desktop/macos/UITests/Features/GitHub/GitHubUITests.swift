import Foundation
import XCTest

@MainActor
final class GitHubUITests: XCTestCase {

  override func setUpWithError() throws {
    continueAfterFailure = false
  }

  func testGitHubOnboardingUsesTheBundledBackendAndReadOnlyFixture() throws {
    let environment = ProcessInfo.processInfo.environment
    guard let port = environment["CTXFAB_GITHUB_TEST_PORT"],
      let hitsPath = environment["CTXFAB_GITHUB_TEST_HITS"]
    else { throw XCTSkip("Run make app-test to start the loopback GitHub fixture.") }

    let app = XCUIApplication()
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = directory.path
    app.launchEnvironment["CTXFAB_TEST_GITHUB_FIXTURE_PORT"] = port
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    app.launch()

    let hits = URL(filePath: hitsPath)
    let baseline =
      (try? String(contentsOf: hits, encoding: .utf8).split(separator: "\n").count) ?? 0
    connect(in: app)
    try addRepository(in: app, hits: hits)
    try recoverAccess(in: app, hits: hits)
    try verifyToolbar(in: app, hits: hits)
    try verifyTraffic(in: app, hits: hits, baseline: baseline)
    disconnect(in: app)
  }

  func testGitHubBranchPickerSearchesLoadedPagesAndUpdatesTheDraft() async throws {
    guard let port = ProcessInfo.processInfo.environment["CTXFAB_GITHUB_TEST_PORT"]
    else { throw XCTSkip("Run with the loopback GitHub fixture.") }
    addTeardownBlock {
      try await Self.fixtureControl(port: port, route: "hold-branches", method: "DELETE")
    }
    let data = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = data.path
    app.launchEnvironment["CTXFAB_TEST_GITHUB_FIXTURE_PORT"] = port
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: data)
    }
    app.launch()
    connect(in: app)
    openFirstRepositoryOptions(in: app)
    app.buttons["github-save"].click()
    let repository = app.buttons["github-edit-41"]
    XCTAssertTrue(repository.waitForExistence(timeout: 8))
    repository.click()

    let picker = app.buttons["github-branch-picker"]
    XCTAssertTrue(picker.wait(for: \.isEnabled, toEqual: true, timeout: 5))
    let search = app.textFields["github-branch-search"]
    try await Self.fixtureControl(port: port, route: "hold-branches", method: "POST")
    picker.click()
    XCTAssertTrue(element("github-branch-loading", in: app).waitForExistence(timeout: 5))
    let searchFrameDuringLoad = search.frame
    capture("GitHub branches loading", in: app)
    try await Self.fixtureControl(port: port, route: "hold-branches", method: "DELETE")
    XCTAssertTrue(app.buttons["github-branch-option-main"].waitForExistence(timeout: 5))
    XCTAssertEqual(search.frame, searchFrameDuringLoad)
    search.click()
    search.typeText("release")
    XCTAssertTrue(element("github-branch-empty", in: app).waitForExistence(timeout: 4))
    let release = app.buttons["github-branch-option-release/v1"]
    XCTAssertFalse(release.exists)
    app.buttons["github-branch-load-more"].click()
    XCTAssertTrue(release.waitForExistence(timeout: 5))
    capture("GitHub branch search result", in: app)
    release.click()
    XCTAssertTrue(search.waitForNonExistence(timeout: 4))
    XCTAssertEqual(app.textFields["github-branch"].value as? String, "release/v1")
    XCTAssertEqual(element("github-current-code", in: app).label, "Branch · release/v1")
  }

  func testGitHubRefreshPublishesAndShowsANewCommit() async throws {
    let environment = ProcessInfo.processInfo.environment
    guard let port = environment["CTXFAB_GITHUB_TEST_PORT"]
    else { throw XCTSkip("Run make app-test to start the loopback GitHub fixture.") }
    addTeardownBlock { try await Self.fixtureControl(port: port, route: "graph", method: "DELETE") }
    addTeardownBlock {
      try await Self.fixtureControl(port: port, route: "advance-code", method: "DELETE")
    }
    addTeardownBlock {
      try await Self.fixtureControl(port: port, route: "hold-code-tree", method: "DELETE")
    }
    try await Self.fixtureControl(port: port, route: "graph", method: "POST")
    let data = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_GITHUB_FIXTURE_PORT"] = port
    app.launchEnvironment["CTXFAB_TEST_GITHUB_SCHEDULED"] = "1"
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: data)
    }
    _ = try stageAnswerWorker(in: data, app: app)
    app.launch()
    connect(in: app)
    openFirstRepositoryOptions(in: app)
    let graphOption = element("github-code-graph", in: app)
    XCTAssertTrue(graphOption.waitForExistence(timeout: 5))
    graphOption.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.5)).click()
    app.buttons["github-save"].click()
    let row = app.buttons["github-edit-41"]
    XCTAssertTrue(row.waitForExistence(timeout: 6))
    row.click()

    XCTAssertTrue(commit(in: app, character: "a").waitForExistence(timeout: 10))
    checkGraph(in: app, resolution: "resolved")
    try await Self.fixtureControl(port: port, route: "advance-code", method: "POST")
    let refresh = app.buttons["github-refresh-code"]
    XCTAssertTrue(refresh.wait(for: \.isEnabled, toEqual: true, timeout: 5))
    XCTAssertTrue(refresh.isHittable)
    try await Self.fixtureControl(port: port, route: "hold-code-tree", method: "POST")
    refresh.click()
    let endpoint = try BackendEndpoint.read(in: data)
    let traces = try await joinGitHubRefresh(in: endpoint)
    try await Self.fixtureControl(port: port, route: "hold-code-tree", method: "DELETE")
    XCTAssertTrue(commit(in: app, character: "b").waitForExistence(timeout: 10))
    capture("GitHub refreshed commit", in: app)
    checkGraph(in: app, resolution: "unresolved")
    app.buttons["github-settings-done"].click()
    checkCitedAnswer(in: app)
    inspectJoinedCollection(traces, in: app)
  }

  private struct CollectionTraces {
    let request: String
    let work: String
  }

  private func joinGitHubRefresh(in endpoint: BackendEndpoint) async throws -> CollectionTraces {
    struct Repositories: Decodable {
      struct Entry: Decodable {
        struct Collection: Decodable { let phase: String }
        let codeCollection: Collection?
      }
      let repositories: [Entry]
    }
    var read = URLRequest(url: endpoint.url.appending(path: "v1/github"))
    read.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let deadline = ContinuousClock.now + .seconds(4)
    while ContinuousClock.now < deadline {
      let (data, response) = try await URLSession.shared.data(for: read)
      XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
      let state = try JSONDecoder().decode(Repositories.self, from: data)
      if state.repositories.first?.codeCollection?.phase == "running" {
        var request = URLRequest(
          url: endpoint.url.appending(path: "v1/github/repositories/41/refresh")
            .appending(queryItems: [.init(name: "background", value: "true")]))
        request.httpMethod = "POST"
        request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
        let (_, response) = try await URLSession.shared.data(for: request)
        let result = try XCTUnwrap(response as? HTTPURLResponse)
        XCTAssertEqual(result.statusCode, 202)
        let http = try XCTUnwrap(result.value(forHTTPHeaderField: "X-Context-Trace-Id"))
        let work = try XCTUnwrap(result.value(forHTTPHeaderField: "X-Context-Work-Trace-Id"))
        XCTAssertNotEqual(http, work)
        return .init(request: http, work: work)
      }
      try await Task.sleep(for: .milliseconds(50))
    }
    return try XCTUnwrap(nil, "The held GitHub refresh did not start.")
  }

  private func inspectJoinedCollection(_ traces: CollectionTraces, in app: XCUIApplication) {
    app.selectWorkspace("diagnostics")
    let lookup = app.buttons["open-diagnostic-trace"]
    XCTAssertTrue(lookup.waitForExistence(timeout: 5))
    lookup.click()
    let id = app.textFields["diagnostic-trace-id"]
    XCTAssertTrue(id.waitForExistence(timeout: 5))
    id.click()
    for character in traces.request {
      id.typeKey(String(character), modifierFlags: [])
    }
    let entered = id.value as? String
    XCTAssertEqual(entered, traces.request)
    guard entered == traces.request else {
      return
    }
    app.buttons["diagnostic-open-trace"].click()
    assertTraceText(traces.request, identifier: "trace-inspector-id", in: app)
    let collection = app.buttons["trace-view-collection-trace"]
    XCTAssertTrue(collection.waitForExistence(timeout: 5))
    collection.click()
    assertTraceText(traces.work, identifier: "trace-inspector-id", in: app)
    XCTAssertTrue(
      app.staticTexts["trace-step-github_current_collection"].waitForExistence(timeout: 5))
    let provider = app.staticTexts["trace-step-github_provider_request"].firstMatch
    XCTAssertTrue(provider.waitForExistence(timeout: 5))
    provider.click()
    assertTraceText(
      "GET /repos/{owner}/{repo}", identifier: "trace-span-http-request", in: app)
    assertTraceText("200", identifier: "trace-span-http-status", in: app)
    captureWindow("GitHub provider request trace", in: app)
    app.buttons["trace-inspector-back"].click()
    assertTraceText(traces.request, identifier: "trace-inspector-id", in: app)
    app.buttons["trace-inspector-close"].click()
  }

  func testHistoryOnlyRepositoryResumesFromRepositorySettings() async throws {
    let port = try XCTUnwrap(ProcessInfo.processInfo.environment["CTXFAB_GITHUB_TEST_PORT"])
    addTeardownBlock {
      try await Self.fixtureControl(port: port, route: "history", method: "DELETE")
    }
    try await Self.fixtureControl(port: port, route: "history", method: "POST")
    let app = XCUIApplication()
    let data = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = data.path
    app.launchEnvironment["CTXFAB_TEST_GITHUB_FIXTURE_PORT"] = port
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: data)
    }
    app.launch()
    connect(in: app)
    openFirstRepositoryOptions(in: app)
    let current = element("github-current-code", in: app)
    XCTAssertTrue(current.waitForExistence(timeout: 5))
    current.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.5)).click()
    element("github-history", in: app)
      .coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.5)).click()
    try prepareRefreshInterval(in: app)
    app.buttons["github-save"].click()
    let row = app.buttons["github-edit-41"]
    XCTAssertTrue(row.waitForExistence(timeout: 6))
    row.click()

    let savedInterval = app.textFields["github-interval"]
    _ = try XCTUnwrap(savedInterval.waitForExistence(timeout: 5) ? savedInterval : nil)
    XCTAssertEqual(savedInterval.value as? String, "15")
    let refresh = app.buttons["github-refresh-history"]
    XCTAssertTrue(refresh.wait(for: \.isEnabled, toEqual: true, timeout: 5))
    XCTAssertFalse(app.buttons["github-refresh-code"].isEnabled)
    let paused = app.otherElements["github-data-pause-state"]
    XCTAssertFalse(paused.exists)
    let refreshed = app.staticTexts["Code history refreshed."]
    refresh.click()
    XCTAssertTrue(refreshed.waitForExistence(timeout: 8))
    try inspectCollectionTrace("history", in: app)
    try purgeAndResumeHistory(in: app, refresh: refresh, refreshed: refreshed, paused: paused)
  }

  private func purgeAndResumeHistory(
    in app: XCUIApplication, refresh: XCUIElement, refreshed: XCUIElement, paused: XCUIElement
  ) throws {
    let form = app.scrollViews["github-settings-status-form"]
    let purge = app.buttons["github-purge-data"]
    let compact = !purge.isHittable
    if compact { form.scroll(byDeltaX: 0, deltaY: -400) }
    _ = try XCTUnwrap(purge.wait(for: \.isHittable, toEqual: true, timeout: 5) ? purge : nil)
    XCTAssertTrue(purge.wait(for: \.isEnabled, toEqual: true, timeout: 5))
    purge.click()
    let confirm = app.buttons["Delete Local Data"]
    XCTAssertTrue(confirm.wait(for: \.isHittable, toEqual: true, timeout: 5))
    confirm.click()
    _ = try XCTUnwrap(
      paused.wait(for: \.label, toEqual: "Automatic collection paused", timeout: 8)
        ? paused : nil)
    capture("History-only repository paused", in: app)
    if !refresh.isHittable { form.scroll(byDeltaX: 0, deltaY: 400) }
    XCTAssertTrue(refresh.wait(for: \.isHittable, toEqual: true, timeout: 5))
    XCTAssertTrue(refresh.isEnabled)
    refresh.click()
    XCTAssertTrue(refreshed.waitForExistence(timeout: 8))
    XCTAssertTrue(paused.waitForNonExistence(timeout: 4))
    capture("History-only repository after resuming", in: app)
  }

  private func prepareRefreshInterval(in app: XCUIApplication) throws {
    let field = app.textFields["github-interval"]
    _ = try XCTUnwrap(field.waitForExistence(timeout: 5) ? field : nil)
    field.click()
    app.typeKey("a", modifierFlags: .command)
    field.typeText("10")
    XCTAssertEqual(field.value as? String, "10")
    let range = app.staticTexts["github-interval-range"]
    XCTAssertTrue(range.exists)
    XCTAssertTrue((range.value as? String ?? range.label).contains("15 to 1440 minutes"))
    let save = app.buttons["github-save"]
    XCTAssertFalse(save.isEnabled)
    capture("GitHub refresh interval validation", in: app)
    app.typeKey("a", modifierFlags: .command)
    field.typeText("15")
    _ = try XCTUnwrap(save.wait(for: \.isEnabled, toEqual: true, timeout: 5) ? save : nil)
  }

  private func openFirstRepositoryOptions(in app: XCUIApplication) {
    app.buttons["github-add"].click()
    XCTAssertTrue(element("github-candidate-41", in: app).waitForExistence(timeout: 5))
    app.typeKey(.downArrow, modifierFlags: [])
    let next = app.buttons["github-continue"]
    XCTAssertTrue(next.wait(for: \.isEnabled, toEqual: true, timeout: 5))
    app.typeKey(.return, modifierFlags: [])
  }

  private func checkCitedAnswer(in app: XCUIApplication) {
    app.selectWorkspace("answers")
    let question = element("answer-question", in: app)
    XCTAssertTrue(question.waitForExistence(timeout: 5))
    question.click()
    question.typeText("graph_example")
    XCTAssertEqual(question.value as? String, "graph_example")
    XCTAssertFalse(element("answer-code-scope", in: app).exists)
    let ask = app.buttons["Ask locally"]
    XCTAssertTrue(ask.wait(for: \.isEnabled, toEqual: true, timeout: 10))
    ask.click()
    let citation = app.links["answer-citation"]
    XCTAssertTrue(citation.waitForExistence(timeout: 12), app.debugDescription)
    XCTAssertEqual(citation.label, "src/lib.rs#L1-L2 @ bbbbbbbbbb")
    XCTAssertTrue(app.staticTexts["The call is in src/lib.rs."].exists)
    let excerpt = "fn call(){crate::target::graph_example();}"
    let quote = app.staticTexts.matching(
      NSPredicate(format: "label CONTAINS %@ OR value CONTAINS %@", excerpt, excerpt)
    ).firstMatch
    XCTAssertTrue(quote.exists, app.debugDescription)
    capture("Answer with refreshed GitHub citation", in: app)
  }

  private func checkGraph(in app: XCUIApplication, resolution: String) {
    let browse = app.buttons["github-browse-graph"]
    XCTAssertTrue(browse.waitForExistence(timeout: 5))
    browse.click()
    let field = app.textFields["github-graph-symbol"]
    XCTAssertTrue(field.waitForExistence(timeout: 5))
    field.click()
    field.typeText("graph_example")
    app.buttons["github-graph-find"].click()
    XCTAssertTrue(element("github-graph-call-\(resolution)", in: app).waitForExistence(timeout: 8))
    XCTAssertTrue(app.links["src/lib.rs#L2"].exists)
    capture("GitHub graph \(resolution)", in: app)
    app.buttons["github-graph-back"].click()
  }

  func testActionsActivityShowsObservedRun() async throws {
    let environment = ProcessInfo.processInfo.environment
    guard let port = environment["CTXFAB_GITHUB_TEST_PORT"]
    else { throw XCTSkip("Run with the loopback GitHub fixture.") }
    addTeardownBlock {
      try await Self.fixtureControl(port: port, route: "actions", method: "DELETE")
    }
    try await Self.fixtureControl(port: port, route: "actions", method: "POST")
    let data = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = data.path
    app.launchEnvironment["CTXFAB_TEST_GITHUB_FIXTURE_PORT"] = port
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: data)
    }
    app.launch()
    try openActionsWorkspace(in: app)
    try checkActionsWorkspace(in: app)
    app.buttons["github-actions-back"].click()
    let schedule = element("github-actions-next-check", in: app)
    XCTAssertTrue(schedule.waitForExistence(timeout: 5))
    try inspectCollectionTrace("actions", in: app)
  }

  private func openActionsWorkspace(in app: XCUIApplication) throws {
    connect(in: app)
    app.buttons["github-add"].click()
    let candidate = element("github-candidate-41", in: app)
    _ = try XCTUnwrap(candidate.waitForExistence(timeout: 5) ? candidate : nil)
    candidate.click()
    app.buttons["github-continue"].click()
    let actions = element("github-actions", in: app)
    _ = try XCTUnwrap(actions.waitForExistence(timeout: 5) ? actions : nil)
    actions.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.5)).click()
    app.buttons["github-save"].click()
    let row = app.buttons["github-edit-41"]
    _ = try XCTUnwrap(row.waitForExistence(timeout: 8) ? row : nil)
    row.click()
    let view = app.buttons["github-view-actions"]
    _ = try XCTUnwrap(view.waitForExistence(timeout: 5) ? view : nil)
    let sheet = app.sheets.firstMatch
    XCTAssertGreaterThanOrEqual(sheet.frame.width, 760)
    XCTAssertTrue(element("github-branch", in: app).isHittable)
    XCTAssertEqual(element("github-current-code", in: app).label, "Branch · main")
    XCTAssertTrue(element("github-contents-access", in: app).isHittable)
    let refresh = app.buttons["github-refresh-repository-actions"]
    _ = try XCTUnwrap(refresh.wait(for: \.isEnabled, toEqual: true, timeout: 5) ? refresh : nil)
    capture("GitHub repository settings workspace", in: app)
    refresh.click()
    let refreshed = app.staticTexts["Workflow runs refreshed."]
    _ = try XCTUnwrap(refreshed.waitForExistence(timeout: 12) ? refreshed : nil)
    view.click()
  }

  private func checkActionsWorkspace(in app: XCUIApplication) throws {
    let refresh = app.buttons["github-refresh-actions"]
    _ = try XCTUnwrap(refresh.wait(for: \.isEnabled, toEqual: true, timeout: 5) ? refresh : nil)
    let sheet = app.sheets.firstMatch
    let actionsSize = sheet.frame.size
    XCTAssertGreaterThanOrEqual(actionsSize.width, 760)
    XCTAssertFalse(element("github-actions-next-check", in: app).exists)
    let run = app.otherElements["github-action-run-30-1"]
    _ = try XCTUnwrap(run.waitForExistence(timeout: 12) ? run : nil)
    let observation = "CI · Run 30 · attempt 1 · queued · aaaaaaaaaa"
    _ = try XCTUnwrap(run.wait(for: \.label, toEqual: observation, timeout: 4) ? run : nil)
    XCTAssertTrue(app.links["github-action-link-30-1"].exists)
    XCTAssertEqual(sheet.frame.size, actionsSize)
    capture("GitHub Actions attempt", in: app)
    let otherWorkflow = app.buttons["github-action-workflow-21"]
    _ = try XCTUnwrap(otherWorkflow.waitForExistence(timeout: 5) ? otherWorkflow : nil)
    otherWorkflow.click()
    _ = try XCTUnwrap(
      run.waitForNonExistence(timeout: 4) ? otherWorkflow : nil,
      "A run from another workflow must disappear when the filter changes")
    app.buttons["github-action-workflow-20"].click()
    _ = try XCTUnwrap(run.waitForExistence(timeout: 4) ? run : nil)
    XCTAssertEqual(sheet.frame.size, actionsSize)
  }

  private func inspectCollectionTrace(_ feature: String, in app: XCUIApplication) throws {
    let trace = app.buttons["github-\(feature)-trace-collection"]
    _ = try XCTUnwrap(trace.waitForExistence(timeout: 8) ? trace : nil)
    trace.click()
    let outline = app.staticTexts["trace-step-github_\(feature)_collection"]
    _ = try XCTUnwrap(outline.waitForExistence(timeout: 8) ? outline : nil)
    XCTAssertFalse(element("trace-inspector-answer-pane", in: app).exists)
    let provider = app.staticTexts["trace-step-github_provider_request"].firstMatch
    _ = try XCTUnwrap(provider.waitForExistence(timeout: 5) ? provider : nil)
    provider.click()
    assertTraceText(
      "GET /repos/{owner}/{repo}", identifier: "trace-span-http-request", in: app)
    assertTraceText("200", identifier: "trace-span-http-status", in: app)
    captureWindow("GitHub \(feature) collection trace", in: app)
    app.buttons["trace-inspector-close"].click()
    XCTAssertTrue(outline.waitForNonExistence(timeout: 5))
  }

  nonisolated private static func fixtureControl(port: String, route: String, method: String)
    async throws
  {
    guard let value = UInt16(port), value > 0,
      let url = URL(string: "http://127.0.0.1:\(value)/__ui/\(route)")
    else { throw URLError(.badURL) }
    var request = URLRequest(url: url)
    request.httpMethod = method
    request.timeoutInterval = 5
    let (_, response) = try await URLSession.shared.data(for: request)
    guard let response = response as? HTTPURLResponse, response.statusCode == 204 else {
      throw URLError(.badServerResponse)
    }
  }

  private func commit(in app: XCUIApplication, character: String) -> XCUIElement {
    app.staticTexts.matching(
      NSPredicate(
        format: "identifier == %@ AND value == %@",
        "github-indexed-commit", String(repeating: character, count: 10)
      )
    ).firstMatch
  }

  private func connect(in app: XCUIApplication) {
    XCTAssertTrue(element("backend-ready", in: app).waitForExistence(timeout: 12))
    app.selectWorkspace("connectors")
    let manage = app.buttons["github-manage"]
    XCTAssertTrue(manage.waitForExistence(timeout: 5))
    captureWindow("Connectors", in: app)
    manage.click()
    app.buttons["github-connect"].click()
    capture("GitHub account setup", in: app)
    submitToken("fixture_primary", in: app)
    XCTAssertTrue(app.buttons["github-add"].wait(for: \.isEnabled, toEqual: true, timeout: 8))
    capture("Empty repository list", in: app)
  }

  private func addRepository(in app: XCUIApplication, hits: URL) throws {
    app.buttons["github-add"].click()
    XCTAssertTrue(element("github-candidate-41", in: app).waitForExistence(timeout: 5))
    let next = app.buttons["github-continue"]
    app.typeKey(.downArrow, modifierFlags: [])
    XCTAssertTrue(next.wait(for: \.isEnabled, toEqual: true, timeout: 5))

    app.buttons["github-load-more"].click()
    XCTAssertTrue(element("github-candidate-42", in: app).waitForExistence(timeout: 5))
    capture("Repository picker", in: app)

    try pickBetaWithKeyboard(in: app, hits: hits)
    let actions = element("github-actions", in: app)
    XCTAssertTrue(actions.waitForExistence(timeout: 5))
    capture("GitHub add repository options", in: app)
    actions.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.5)).click()
    app.buttons["github-save"].click()
    let row = app.buttons["github-edit-42"]
    XCTAssertTrue(row.waitForExistence(timeout: 8))
    XCTAssertEqual(row.value as? String, "Actions access unverified")
    capture("GitHub repositories", in: app)
  }

  private func pickBetaWithKeyboard(in app: XCUIApplication, hits: URL) throws {
    let before = try Data(contentsOf: hits)
    let filter = app.textFields["github-picker-search"]
    let branch = app.textFields["github-branch"]
    let back = app.buttons["github-picker-back"]
    filter.click()
    filter.typeKey(.downArrow, modifierFlags: [])
    app.typeKey(.return, modifierFlags: [])
    XCTAssertTrue(branch.waitForExistence(timeout: 5))
    XCTAssertEqual(branch.value as? String, "trunk")

    back.click()
    filter.click()
    filter.typeText("beta")
    XCTAssertTrue(element("github-candidate-41", in: app).waitForNonExistence(timeout: 5))
    app.typeKey(.return, modifierFlags: [])
    XCTAssertTrue(branch.waitForExistence(timeout: 5))
    back.click()
    XCTAssertEqual(filter.value as? String, "beta")
    XCTAssertTrue(app.buttons["github-continue"].isEnabled)
    XCTAssertEqual(try Data(contentsOf: hits), before)
    app.typeKey(.escape, modifierFlags: [])
    XCTAssertTrue(filter.waitForNonExistence(timeout: 5))

    app.buttons["github-add"].click()
    XCTAssertTrue(filter.waitForExistence(timeout: 5))
    filter.click()
    filter.typeKey(.upArrow, modifierFlags: [])
    app.typeKey(.return, modifierFlags: [])
    XCTAssertTrue(branch.waitForExistence(timeout: 5))
    XCTAssertEqual(branch.value as? String, "trunk")
  }

  private func recoverAccess(in app: XCUIApplication, hits: URL) throws {
    let row = app.buttons["github-edit-42"]
    row.click()
    let branch = app.textFields["github-branch"]
    let status = element("github-actions-access", in: app)
    let recheck = app.buttons["github-verify-access"]
    XCTAssertTrue(branch.waitForExistence(timeout: 5))
    XCTAssertEqual(status.label, "GitHub Actions, Access unverified")
    capture("Repository access unverified", in: app)

    branch.click()
    branch.typeKey("a", modifierFlags: .command)
    branch.typeText("draft")
    app.buttons["github-repository-account"].click()
    submitToken("r", in: app)
    XCTAssertTrue(recheck.waitForExistence(timeout: 8), app.debugDescription)
    XCTAssertEqual(branch.value as? String, "draft")
    recheck.click()

    let verified = app.descendants(matching: .any).matching(
      NSPredicate(
        format: "identifier == %@ AND label == %@",
        "github-actions-access", "GitHub Actions, Read access verified")
    ).firstMatch
    XCTAssertTrue(verified.waitForExistence(timeout: 8))
    XCTAssertEqual(branch.value as? String, "draft")
    let requests = try String(contentsOf: hits, encoding: .utf8).split(separator: "\n")
    XCTAssertEqual(
      requests.filter { $0 == "GET /repos/fixture/beta/actions/runs?per_page=1" }.count, 2)
    XCTAssertEqual(requests.filter { $0 == "GET /repos/fixture/beta/branches/trunk" }.count, 2)
    XCTAssertFalse(requests.contains("GET /repos/fixture/beta/branches/draft"))
    capture("Repository access verified", in: app)
    app.buttons["Cancel"].click()
    row.click()
    XCTAssertEqual(branch.value as? String, "trunk")
    app.buttons["github-settings-done"].click()
  }

  private func verifyToolbar(in app: XCUIApplication, hits: URL) throws {
    let before = try Data(contentsOf: hits)
    let row = app.buttons["github-edit-42"]
    XCTAssertTrue(app.buttons["github-reload"].exists)
    app.typeKey("f", modifierFlags: .command)
    let search = app.searchFields.firstMatch
    XCTAssertTrue(search.waitForExistence(timeout: 5))
    search.typeText("xyz")
    XCTAssertTrue(row.waitForNonExistence(timeout: 5))
    captureWindow("GitHub repository filter empty", in: app)
    search.typeKey("a", modifierFlags: .command)
    search.typeKey(.delete, modifierFlags: [])
    XCTAssertTrue(row.waitForExistence(timeout: 5))
    XCTAssertEqual(try Data(contentsOf: hits), before)
  }

  private func disconnect(in app: XCUIApplication) {
    app.buttons["github-account"].click()
    app.buttons["github-disconnect"].click()
    let confirm = app.sheets.buttons["Disconnect"]
    XCTAssertTrue(confirm.waitForExistence(timeout: 5))
    confirm.click()
    XCTAssertTrue(app.buttons["github-add"].wait(for: \.isEnabled, toEqual: false, timeout: 5))
    XCTAssertTrue(app.buttons["github-edit-42"].exists)
    app.buttons["github-back"].click()
    XCTAssertTrue(app.buttons["github-manage"].waitForExistence(timeout: 5))
  }

  private func verifyTraffic(in app: XCUIApplication, hits: URL, baseline: Int) throws {
    let requests = try String(contentsOf: hits, encoding: .utf8).split(separator: "\n")
    app.buttons["github-requests"].click()
    let attempts = element("github-request-attempts", in: app)
    XCTAssertTrue(attempts.waitForExistence(timeout: 5))
    XCTAssertEqual(attempts.label, "Attempts, \((requests.count - baseline).formatted())")
    let rate = app.textFields["github-request-rate"]
    rate.click()
    rate.typeKey("a", modifierFlags: .command)
    rate.typeText("30")
    XCTAssertEqual(rate.value as? String, "30")
    let apply = app.buttons["github-apply-rate"]
    XCTAssertTrue(apply.wait(for: \.isHittable, toEqual: true, timeout: 5))
    XCTAssertTrue(apply.wait(for: \.isEnabled, toEqual: true, timeout: 5))
    apply.click()
    let applied = app.descendants(matching: .any).matching(
      NSPredicate(
        format: "identifier == %@ AND label == %@",
        "github-effective-rate", "Effective limit, 30 requests per minute")
    ).firstMatch
    XCTAssertTrue(applied.waitForExistence(timeout: 5))
    XCTAssertEqual(try String(contentsOf: hits, encoding: .utf8).split(separator: "\n"), requests)
    capture("GitHub request activity", in: app)
    app.buttons["Done"].click()
  }

  private func submitToken(_ suffix: String, in app: XCUIApplication) {
    let token = app.secureTextFields["github-token"]
    XCTAssertTrue(token.waitForExistence(timeout: 5))
    token.click()
    token.typeText("github_pat_")
    token.typeText(suffix)
    app.buttons["github-authorize"].click()
  }

  private func element(_ identifier: String, in app: XCUIApplication) -> XCUIElement {
    app.descendants(matching: .any).matching(identifier: identifier).firstMatch
  }

  private func capture(_ name: String, in app: XCUIApplication) {
    captureWindow(name, in: app)
  }
}
