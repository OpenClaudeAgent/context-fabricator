import Foundation
import XCTest

extension XCTestCase {
  @MainActor
  func stageAnswerWorker(in directory: URL, app: XCUIApplication) throws -> URL {
    let assets = directory.appending(path: "inference")
    try FileManager.default.createDirectory(
      at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    try FileManager.default.createDirectory(at: assets, withIntermediateDirectories: false)
    let worker = try XCTUnwrap(
      Bundle(for: type(of: self)).url(
        forResource: "llama-server", withExtension: nil))
    try FileManager.default.createSymbolicLink(
      at: assets.appending(path: "llama-server"), withDestinationURL: worker)
    try Data().write(to: assets.appending(path: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"))
    app.launchEnvironment["CTXFAB_TEST_DATA_DIRECTORY"] = directory.path
    app.launchEnvironment["CTXFAB_INFERENCE_DIRECTORY"] = assets.path
    return assets
  }
}
