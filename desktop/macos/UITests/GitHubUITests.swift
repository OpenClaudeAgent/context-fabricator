import Foundation
import XCTest

@MainActor
final class GitHubUITests: XCTestCase {
  func testGitHubOnboardingUsesTheBundledBackendAndReadOnlyFixture() throws {
    continueAfterFailure = false
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

  func testGitHubRefreshPublishesAndShowsANewCommit() throws {
    continueAfterFailure = false
    let environment = ProcessInfo.processInfo.environment
    guard let port = environment["CTXFAB_GITHUB_TEST_PORT"]
    else { throw XCTSkip("Run make app-test to start the loopback GitHub fixture.") }
    let data = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = data.path
    app.launchEnvironment["CTXFAB_TEST_GITHUB_FIXTURE_PORT"] = port
    app.launchEnvironment["CTXFAB_TEST_GITHUB_SCHEDULED"] = "1"
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: data)
    }
    app.launch()
    connect(in: app)
    app.buttons["github-add"].click()
    let candidate = element("github-candidate-41", in: app)
    XCTAssertTrue(candidate.waitForExistence(timeout: 5))
    candidate.click()
    app.buttons["github-continue"].click()
    app.buttons["github-save"].click()
    let row = app.buttons["github-edit-41"]
    XCTAssertTrue(row.waitForExistence(timeout: 6))
    row.click()

    XCTAssertTrue(commit(in: app, character: "a").waitForExistence(timeout: 10))
    let refresh = app.buttons["github-refresh-code"]
    XCTAssertTrue(refresh.wait(for: \.isEnabled, toEqual: true, timeout: 5))
    XCTAssertTrue(refresh.isHittable)
    refresh.click()
    XCTAssertTrue(commit(in: app, character: "b").waitForExistence(timeout: 10))
    capture("GitHub refreshed commit", in: app)
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
    app.buttons["Cancel"].click()
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
    app.buttons["github-apply-rate"].click()
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
    let sheet = app.sheets.firstMatch
    let capture = XCTAttachment(
      screenshot: sheet.exists ? sheet.screenshot() : app.windows.firstMatch.screenshot())
    capture.name = name
    capture.lifetime = .keepAlways
    add(capture)
  }
}
