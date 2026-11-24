import Foundation
import XCTest

@MainActor
final class BackendLifecycleUITests: XCTestCase {
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
    captureWindow("Backend unavailable", in: app)
    app.selectWorkspace("answers")
    let picker = app.descendants(matching: .any).matching(identifier: "generation-model-picker")
      .firstMatch
    XCTAssertTrue(picker.waitForExistence(timeout: 5))
    picker.click()
    app.menuItems["Manage Models…"].click()
    XCTAssertTrue(app.buttons["Open Model Folder"].waitForExistence(timeout: 5))
    captureWindow("Answers model library without backend", in: app)
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
    let original = try BackendEndpoint.read(in: directory)
    let termination = kill(original.processId, SIGKILL)
    XCTAssertEqual(termination, 0)
    XCTAssertTrue(app.staticTexts["backend-unavailable"].waitForExistence(timeout: 8))
    XCTAssertTrue(
      app.staticTexts["The local backend exited. Check that its data directory is available."]
        .exists)
    captureWindow("Backend restart available", in: app)

    app.buttons["Restart backend"].click()
    XCTAssertTrue(ready.waitForExistence(timeout: 12))
    let restarted = try BackendEndpoint.read(in: directory)
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
    let original = try BackendEndpoint.read(in: directory)

    app.typeKey("w", modifierFlags: .command)
    XCTAssertTrue(app.windows.firstMatch.waitForNonExistence(timeout: 5))
    XCTAssertEqual(app.state, .runningForeground)
    var request = URLRequest(url: original.url.appending(path: "/v1/status"))
    request.setValue("Bearer \(original.token)", forHTTPHeaderField: "Authorization")
    let (_, response) = try await URLSession.shared.data(for: request)
    XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
    let afterClose = try BackendEndpoint.read(in: directory)
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

  private func enterTwoQuestionLines(in question: XCUIElement) {
    question.click()
    question.typeText("First line")
    question.typeKey(.return, modifierFlags: .shift)
    question.typeText("More detail")
    XCTAssertEqual(question.value as? String, "First line\nMore detail")
  }
}
