import Foundation
import XCTest

@MainActor
final class ModelLibraryUITests: XCTestCase {

  override func setUpWithError() throws {
    continueAfterFailure = false
  }

  private struct Status: Decodable {
    let outboundAttempts: Int
  }

  func testGuidedModelSetupShowsQ5DownloadProgress() async throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    let port = try XCTUnwrap(
      ProcessInfo.processInfo.environment["CTXFAB_HUGGING_FACE_PORT"], "Run make app-test.")
    app.launchEnvironment["CTXFAB_TEST_HUGGING_FACE_PORT"] = port
    _ = try launchLibrary(in: app, directory: directory, bundledRuntime: true)
    _ = try BackendEndpoint.read(in: directory)
    let setup = app.buttons["set-up-answers"]
    XCTAssertTrue(setup.wait(for: \.isHittable, toEqual: true, timeout: 10))
    setup.click()
    selectQ5ForSetup(in: app)
    startQ5Download(in: app)
    verifyQ5DownloadProgress(0, in: app)
    captureWindow("Q5 download awaiting first bytes", in: app)
    try await releaseQ5Download(port: port)
    verifyQ5DownloadProgress(1_048_576, in: app)
    verifyDownloadControls(in: app)
  }

  func testManageModelsAlignsRuntimeAndIndexControls() throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchLibrary(in: app, directory: directory, bundledRuntime: true)
    let manage = app.buttons["manage-models"]
    XCTAssertTrue(manage.waitForExistence(timeout: 5))
    manage.click()
    let engine = app.staticTexts["runtime-engine-name"]
    XCTAssertTrue(engine.waitForExistence(timeout: 10), app.debugDescription)
    XCTAssertEqual(engine.value as? String ?? engine.label, "llama.cpp")
    let build = app.staticTexts["runtime-engine-build"]
    XCTAssertTrue(build.exists)
    XCTAssertTrue((build.value as? String ?? build.label).hasPrefix("Build "))
    let index = app.checkBoxes["embedding-index-enabled"]
    XCTAssertTrue(index.waitForExistence(timeout: 5), app.debugDescription)
    XCTAssertTrue([0, 1].contains(index.value as? Int ?? -1))
    let delete = app.buttons["embedding-index-delete"]
    XCTAssertTrue(delete.exists)
    XCTAssertFalse(index.frame.isEmpty)
    XCTAssertFalse(delete.frame.isEmpty)
    XCTAssertEqual(index.frame.maxX, delete.frame.maxX, accuracy: 3)
    XCTAssertEqual(build.frame.maxX, delete.frame.maxX, accuracy: 3)
    let modelDownload = app.buttons["download-model-qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"]
    XCTAssertTrue(modelDownload.exists)
    XCTAssertEqual(modelDownload.frame.maxX, delete.frame.maxX, accuracy: 3)
    let sheet = app.sheets.firstMatch
    sheet.scrollViews["model-library-content"].scroll(byDeltaX: 0, deltaY: -600)
    captureWindow("Manage Models right-aligned controls", in: app)
    let expectedBuild = build.value as? String ?? build.label
    app.buttons["Set Up Answers…"].click()
    XCTAssertEqual(assertSetupEngine(in: app), expectedBuild)
  }

  private func releaseQ5Download(port: String) async throws {
    let url = try XCTUnwrap(URL(string: "http://127.0.0.1:\(port)/__ui/release-q5"))
    var request = URLRequest(url: url)
    request.httpMethod = "POST"
    let (_, response) = try await URLSession.shared.data(for: request)
    XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 204)
  }

  private func selectQ5ForSetup(in app: XCUIApplication) {
    _ = assertSetupEngine(in: app)
    let q4 = app.radioButtons.matching(
      NSPredicate(format: "label CONTAINS %@", "Qwen3 4B · Q4_K_M")
    ).firstMatch
    let q5 = app.radioButtons.matching(
      NSPredicate(format: "label CONTAINS %@", "Qwen3 4B · Q5_K_M")
    ).firstMatch
    XCTAssertTrue(q4.waitForExistence(timeout: 10), app.debugDescription)
    XCTAssertTrue(q5.exists)
    XCTAssertTrue(q4.label.contains("Recommended"))
    XCTAssertTrue(
      q4.label.contains(2_497_281_120.formatted(.byteCount(style: .file))))
    XCTAssertTrue(
      q5.label.contains(2_889_514_080.formatted(.byteCount(style: .file))))
    let embedding = semanticSearchToggle(in: app)
    XCTAssertTrue(embedding.exists)
    XCTAssertTrue(embedding.label.contains("Qwen3 Embedding 0.6B"))
    XCTAssertTrue(
      embedding.label.contains(
        639_150_592.formatted(.byteCount(style: .file))))
    q5.click()
    captureWindow("Model setup choices", in: app)
  }

  private func assertSetupEngine(in app: XCUIApplication) -> String {
    let name = app.staticTexts["setup-runtime-engine-name"]
    XCTAssertTrue(name.waitForExistence(timeout: 8), app.debugDescription)
    XCTAssertEqual(name.value as? String ?? name.label, "llama.cpp")
    let build = app.staticTexts["setup-runtime-engine-build"]
    XCTAssertTrue(build.exists)
    let buildLabel = build.value as? String ?? build.label
    XCTAssertTrue(buildLabel.hasPrefix("Build "))
    XCTAssertGreaterThan(buildLabel.count, "Build ".count)
    return buildLabel
  }

  private func semanticSearchToggle(in app: XCUIApplication) -> XCUIElement {
    app.descendants(matching: .any).matching(identifier: "setup-optional-embedding").firstMatch
  }

  private func startQ5Download(in app: XCUIApplication) {
    app.buttons["Review Setup"].click()
    let reviewedModel = app.staticTexts["setup-reviewed-answer"]
    XCTAssertTrue(reviewedModel.waitForExistence(timeout: 10))
    XCTAssertEqual(reviewedModel.value as? String ?? reviewedModel.label, "Qwen3 4B · Q5_K_M")
    let estimate = app.descendants(matching: .any).matching(identifier: "setup-space-estimate")
      .firstMatch
    XCTAssertTrue(estimate.waitForExistence(timeout: 10))
    let download = app.buttons["Download and Enable"]
    XCTAssertTrue(download.waitForExistence(timeout: 5))
    XCTAssertTrue(download.isEnabled, app.debugDescription)
    captureWindow("Model setup Q5 estimate", in: app)
    download.click()
  }

  private func verifyQ5DownloadProgress(_ byteCount: Int64, in app: XCUIApplication) {
    let name = "Qwen3 4B · Q5_K_M"
    let active = app.staticTexts.matching(identifier: "setup-active-model")
      .matching(NSPredicate(format: "label CONTAINS %@ OR value CONTAINS %@", name, name))
      .firstMatch
    XCTAssertTrue(active.waitForExistence(timeout: 10), app.debugDescription)
    let source = app.descendants(matching: .any).matching(identifier: "setup-download-source")
      .firstMatch
    XCTAssertTrue(source.exists)
    XCTAssertTrue(source.label.contains("Hugging Face"))
    let progress = app.descendants(matching: .any).matching(identifier: "setup-download-progress")
      .firstMatch
    XCTAssertTrue(progress.waitForExistence(timeout: 20), app.debugDescription)
    let transferred = byteCount.formatted(.byteCount(style: .file))
    let total = 2_889_514_080.formatted(.byteCount(style: .file))
    let expected = "\(transferred) of \(total)"
    let byteProgress = NSPredicate(
      format: "value == %@ OR label == %@", expected, expected)
    let visibleBytes = app.staticTexts.matching(identifier: "setup-progress-bytes")
      .matching(byteProgress).firstMatch
    XCTAssertTrue(visibleBytes.waitForExistence(timeout: 10), app.debugDescription)
    XCTAssertTrue(app.buttons["Continue in Background"].isEnabled)
    if byteCount > 0 { captureWindow("Model setup Q5 download", in: app) }
  }

  private func verifyDownloadControls(in app: XCUIApplication) {
    app.buttons["setup-manage-downloads"].click()
    XCTAssertTrue(app.staticTexts["setup-active-model"].waitForNonExistence(timeout: 5))

    let profile = "qwen3-4b-instruct-2507-q5-k-m-5bde5e9d-v1"
    let downloads = app.staticTexts["model-library-downloads"]
    XCTAssertTrue(downloads.waitForExistence(timeout: 10))
    filterModels("z", in: app)
    XCTAssertTrue(app.staticTexts["No catalog models match your search."].exists)
    let pause = app.buttons["pause-model-\(profile)"]
    XCTAssertTrue(pause.wait(for: \.isHittable, toEqual: true, timeout: 10))
    let progress = app.descendants(matching: .any).matching(
      identifier: "model-download-progress-\(profile)"
    ).firstMatch
    XCTAssertTrue(progress.exists)
    captureWindow("Q5 download in Manage Models", in: app)
    pause.click()
    verifyDownloadStatus(prefix: "Paused", bytes: 1_048_576, profile: profile, in: app)
    XCTAssertTrue(downloads.exists)
    XCTAssertFalse(progress.exists)
    captureWindow("Q5 download paused", in: app)
    app.buttons["Set Up Answers…"].click()
    XCTAssertTrue(app.staticTexts["Download paused"].waitForExistence(timeout: 10))
    XCTAssertFalse(app.descendants(matching: .any)["setup-download-progress"].exists)
    captureWindow("Paused download in setup", in: app)
    app.buttons["setup-manage-downloads"].click()
    let resume = app.buttons["resume-model-\(profile)"]
    XCTAssertTrue(resume.wait(for: \.isHittable, toEqual: true, timeout: 5))
    resume.click()
    verifyDownloadStatus(prefix: "Downloading", bytes: 2_097_152, profile: profile, in: app)
    XCTAssertTrue(pause.isHittable)
    pause.click()
    verifyDownloadStatus(prefix: "Paused", bytes: 2_097_152, profile: profile, in: app)
    app.buttons["Clear Search"].click()
    discardDownload(profile, in: app)
    XCTAssertTrue(downloads.waitForNonExistence(timeout: 5))
  }

  private func verifyDownloadStatus(
    prefix: String, bytes: Int64, profile: String, in app: XCUIApplication
  ) {
    let transferred = bytes.formatted(.byteCount(style: .file))
    let expected = "\(prefix) · \(transferred)"
    let progress = NSPredicate(
      format: "value BEGINSWITH %@ OR label BEGINSWITH %@", expected, expected)
    let status = app.staticTexts.matching(identifier: "model-status-\(profile)")
      .matching(progress).firstMatch
    XCTAssertTrue(status.waitForExistence(timeout: 10), app.debugDescription)
  }

  private func discardDownload(_ profile: String, in app: XCUIApplication) {
    let discard = app.buttons["discard-model-partial-\(profile)"]
    XCTAssertTrue(discard.isHittable)
    discard.click()
    let confirm = app.windows["dashboard"].buttons["Discard Partial Download"]
    XCTAssertTrue(confirm.waitForExistence(timeout: 5))
    confirm.click()
    XCTAssertTrue(app.buttons["download-model-\(profile)"].waitForExistence(timeout: 10))
    XCTAssertFalse(discard.exists)
  }

  func testHuggingFaceCatalogSearchAndModelDeletion() throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    let assets = try launchLibrary(in: app, directory: directory)
    try verifyInstalledModel(in: app, assets: assets)
  }

  func testSetupRespectsTheInferenceStorageLimit() async throws {
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    _ = try launchLibrary(in: app, directory: directory, bundledRuntime: true)
    let endpoint = try BackendEndpoint.read(in: directory)
    let startingAttempts = try await outboundAttempts(endpoint)
    app.selectWorkspace("settings")
    let limit = app.textFields["inference-artifact-limit"]
    XCTAssertTrue(limit.waitForExistence(timeout: 5))
    limit.click()
    limit.typeKey("a", modifierFlags: .command)
    limit.typeText("1")
    app.buttons["Save"].click()
    XCTAssertTrue(app.staticTexts["Settings saved."].waitForExistence(timeout: 5))

    app.selectWorkspace("answers")
    let setup = app.buttons["set-up-answers"]
    XCTAssertTrue(setup.waitForExistence(timeout: 8))
    setup.click()
    let embedding = semanticSearchToggle(in: app)
    XCTAssertTrue(embedding.waitForExistence(timeout: 8))
    XCTAssertEqual(embedding.value as? Int, 0)
    embedding.click()
    XCTAssertEqual(embedding.value as? Int, 1)
    let review = app.buttons["Review Setup"]
    XCTAssertTrue(review.isEnabled)
    review.click()
    let blocker = app.staticTexts["There is not enough available storage for this setup."]
    XCTAssertTrue(blocker.waitForExistence(timeout: 5), app.debugDescription)
    XCTAssertFalse(app.buttons["Download and Enable"].isEnabled)
    let attempts = try await outboundAttempts(endpoint)
    XCTAssertEqual(attempts, startingAttempts)
    captureWindow("Setup exceeds inference storage limit", in: app)
    app.buttons["Cancel"].click()
    XCTAssertTrue(blocker.waitForNonExistence(timeout: 5))
  }

  func testDownloadsDiscoveredHuggingFaceWeightsThroughTheBundledBackend() async throws {
    let environment = ProcessInfo.processInfo.environment
    let port = try XCTUnwrap(environment["CTXFAB_HUGGING_FACE_PORT"], "Run make app-test.")
    let hits = try XCTUnwrap(environment["CTXFAB_HUGGING_FACE_HITS"], "Run make app-test.")
    let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let app = XCUIApplication()
    app.launchEnvironment["CTXFAB_TEST_HUGGING_FACE_PORT"] = port
    defer {
      app.terminate()
      try? FileManager.default.removeItem(at: directory)
    }
    let assets = try launchLibrary(in: app, directory: directory)
    let endpoint = try BackendEndpoint.read(in: directory)
    let previousAttempts = try await outboundAttempts(endpoint)
    let hitFile = URL(filePath: hits)
    let previousHits = try hubRequests(in: hitFile).count
    beginHubDownload(in: app)

    let profile = "hf-b2a9f074c39a9b1fb95ac032b4e07878865e79d792d5c42387b598351f3c0483"
    filterModels("fixture", in: app)
    let delete = app.buttons["delete-model-\(profile)"]
    XCTAssertTrue(delete.wait(for: \.isHittable, toEqual: true, timeout: 15))
    let model = assets.appending(path: "\(profile).gguf")
    XCTAssertEqual(try Data(contentsOf: model), Data("GGUF loopback model catalog fixture\n".utf8))
    let source = app.descendants(matching: .any)
      .matching(identifier: "model-source-\(profile)").firstMatch
    XCTAssertTrue(source.exists)
    XCTAssertTrue(source.label.contains("Hugging Face"))
    captureWindow("Hugging Face model installed", in: app)

    app.buttons["Done"].click()
    XCTAssertTrue(delete.waitForNonExistence(timeout: 5))
    app.selectWorkspace("settings")
    let manageModels = app.buttons["settings-manage-models"]
    XCTAssertTrue(
      manageModels.waitForExistence(timeout: 5), "Settings should offer access to Manage Models.")
    manageModels.click()
    filterModels("fixture", in: app)
    XCTAssertTrue(delete.wait(for: \.isHittable, toEqual: true, timeout: 5))
    delete.click()
    let confirm = app.windows["dashboard"].buttons["Delete Model"]
    XCTAssertTrue(confirm.waitForExistence(timeout: 5))
    confirm.click()
    XCTAssertTrue(delete.waitForNonExistence(timeout: 5))
    XCTAssertTrue(app.buttons["download-model-\(profile)"].waitForExistence(timeout: 5))
    XCTAssertFalse(FileManager.default.fileExists(atPath: model.path))
    let attempts = try await outboundAttempts(endpoint)
    XCTAssertEqual(
      attempts - previousAttempts, try verifyHubRequests(in: hitFile, after: previousHits))
    app.buttons["Done"].click()
    XCTAssertTrue(manageModels.wait(for: \.isHittable, toEqual: true, timeout: 5))
  }

  private func filterModels(_ term: String, in app: XCUIApplication) {
    let search = app.textFields["model-library-search"]
    XCTAssertTrue(search.waitForExistence(timeout: 10))
    search.click()
    search.typeKey(String(term.prefix(1)), modifierFlags: [])
    let remaining = term.dropFirst()
    if !remaining.isEmpty { search.typeText(String(remaining)) }
    XCTAssertEqual(search.value as? String, term)
  }

  private func launchLibrary(
    in app: XCUIApplication, directory: URL, bundledRuntime: Bool = false
  ) throws -> URL {
    let assets = directory.appending(path: "inference")
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = directory.path
    if !bundledRuntime { app.launchEnvironment["CTXFAB_INFERENCE_DIRECTORY"] = assets.path }
    try FileManager.default.createDirectory(
      at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    try FileManager.default.createDirectory(at: assets, withIntermediateDirectories: false)
    app.launch()
    let ready = app.descendants(matching: .any).matching(identifier: "backend-ready").firstMatch
    XCTAssertTrue(ready.waitForExistence(timeout: 12))
    app.selectWorkspace("answers")
    return assets
  }

  private func beginHubDownload(in app: XCUIApplication) {
    let manage = app.buttons["manage-models"]
    XCTAssertTrue(manage.waitForExistence(timeout: 5))
    manage.click()
    let browse = app.buttons["browse-hugging-face"]
    XCTAssertTrue(browse.waitForExistence(timeout: 5))
    XCTAssertTrue(browse.label.contains("Search Models"))
    let sheet = app.sheets.firstMatch
    let libraryHeight = sheet.frame.height
    browse.click()
    let query = app.textFields["hugging-face-query"]
    XCTAssertTrue(query.waitForExistence(timeout: 5))
    XCTAssertEqual(sheet.frame.height, libraryHeight, accuracy: 1)
    captureWindow("Hugging Face model search", in: app)
    query.click()
    // A separate key event keeps XCTest from dropping the leading character.
    query.typeKey("c", modifierFlags: [])
    query.typeText("atalog")
    XCTAssertEqual(query.value as? String, "catalog")
    app.buttons["hugging-face-search"].click()

    let repository = app.buttons["hugging-face-repository-fixture/catalog-qwen"]
    XCTAssertTrue(repository.waitForExistence(timeout: 10), app.debugDescription)
    XCTAssertEqual(sheet.frame.height, libraryHeight, accuracy: 1)
    captureWindow("Hugging Face search results", in: app)
    repository.click()
    let file = app.buttons["hugging-face-file-Qwen3-4B-Instruct-2507-Q4_K_M.gguf"]
    XCTAssertTrue(file.waitForExistence(timeout: 10))
    XCTAssertEqual(sheet.frame.height, libraryHeight, accuracy: 1)
    file.click()
    captureWindow("Hugging Face model discovery", in: app)
    let download = app.buttons["hugging-face-download"]
    XCTAssertTrue(download.isEnabled)
    download.click()
  }

  private func outboundAttempts(_ endpoint: BackendEndpoint) async throws -> Int {
    var request = URLRequest(url: endpoint.url.appending(path: "v1/status"))
    request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
    let (body, response) = try await URLSession.shared.data(for: request)
    XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
    return try JSONDecoder().decode(Status.self, from: body).outboundAttempts
  }

  private func hubRequests(in hits: URL) throws -> [String] {
    if !FileManager.default.fileExists(atPath: hits.path) { return [] }
    return try String(contentsOf: hits, encoding: .utf8).split(separator: "\n").map(String.init)
  }

  private func verifyHubRequests(in hits: URL, after previous: Int) throws -> Int {
    let repository = "fixture/catalog-qwen"
    let revision = String(repeating: "a", count: 40)
    let file = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"
    let attempts = Array(try hubRequests(in: hits).dropFirst(previous))
    XCTAssertEqual(
      attempts,
      [
        "GET /api/models",
        "GET /api/models/\(repository)/revision/\(revision)",
        "GET /api/models/\(repository)/tree/\(revision)",
        "GET /api/models/\(repository)/revision/\(revision)",
        "GET /api/models/\(repository)/tree/\(revision)",
        "GET /\(repository)/resolve/\(revision)/\(file)",
      ])
    return attempts.count
  }

  private func verifyInstalledModel(in app: XCUIApplication, assets: URL) throws {
    let model = assets.appending(path: "Qwen3-Embedding-0.6B-Q8_0.gguf")
    try Data().write(to: model)
    let file = try FileHandle(forWritingTo: model)
    defer { try? file.close() }
    // The download catalog recognizes installed weights by their pinned byte count.
    try file.truncate(atOffset: 639_150_592)
    let manage = app.buttons["manage-models"]
    XCTAssertTrue(manage.waitForExistence(timeout: 5))
    manage.click()
    let profile = "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1"
    let status = app.staticTexts["model-status-\(profile)"]
    XCTAssertTrue(status.waitForExistence(timeout: 5))
    XCTAssertTrue((status.value as? String ?? status.label).hasPrefix("Installed · "))
    XCTAssertFalse(app.buttons["download-model-\(profile)"].exists)
    verifyModelSearchAndDeletion(profile: profile, model: model, in: app)
    app.buttons["Done"].click()
  }

  private func verifyModelSearchAndDeletion(profile: String, model: URL, in app: XCUIApplication) {
    let search = app.textFields["model-library-search"]
    let source = app.descendants(matching: .any)
      .matching(identifier: "model-source-\(profile)").firstMatch
    XCTAssertTrue(search.waitForExistence(timeout: 5))
    XCTAssertTrue(source.exists)
    XCTAssertEqual(source.label, "Hugging Face · Qwen")
    let answerModel = app.staticTexts["Qwen3 4B · Q4_K_M"]
    XCTAssertTrue(answerModel.exists)
    search.click()
    search.typeText("Q8_0")
    XCTAssertTrue(answerModel.waitForNonExistence(timeout: 5))
    XCTAssertTrue(source.exists)
    let delete = app.buttons["delete-model-\(profile)"]
    XCTAssertTrue(delete.isEnabled)
    delete.click()
    let dashboard = app.windows["dashboard"]
    let cancel = dashboard.buttons["Cancel"]
    XCTAssertTrue(cancel.waitForExistence(timeout: 5))
    captureWindow("Delete model confirmation", in: app)
    cancel.click()
    XCTAssertTrue(FileManager.default.fileExists(atPath: model.path))
    delete.click()
    let confirm = dashboard.buttons["Delete Model"]
    XCTAssertTrue(confirm.waitForExistence(timeout: 5))
    confirm.click()
    XCTAssertTrue(app.buttons["download-model-\(profile)"].waitForExistence(timeout: 5))
    XCTAssertFalse(FileManager.default.fileExists(atPath: model.path))
    captureWindow("Model library search and source", in: app)
  }
}
