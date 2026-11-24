import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@Test
func generatedSetupClientTransfersTheReviewedPlanThroughTheLoopbackContract() async throws {
  let portFile = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
  let fixture = Process()
  fixture.executableURL = URL(filePath: "/usr/bin/env")
  let script = URL(filePath: #filePath).deletingLastPathComponent()
    .appending(path: "../../../../../quality/models/setup_api_fixture.py").standardizedFileURL
  fixture.arguments = ["python3", script.path, portFile.path]
  fixture.standardOutput = FileHandle.nullDevice
  fixture.standardError = FileHandle.nullDevice
  try fixture.run()
  defer {
    if fixture.isRunning {
      fixture.terminate()
      fixture.waitUntilExit()
    }
    try? FileManager.default.removeItem(at: portFile)
  }
  let deadline = ContinuousClock.now + .seconds(5)
  while !FileManager.default.fileExists(atPath: portFile.path), ContinuousClock.now < deadline {
    try await Task.sleep(for: .milliseconds(25))
  }
  let port = try String(contentsOf: portFile, encoding: .utf8)
  let url = try #require(URL(string: "http://127.0.0.1:\(port)"))
  try await verifySetupContract(SessionClient(url: url, token: "swift-setup-fixture"))
}

private func verifySetupContract(_ client: SessionClient) async throws {
  #expect(try await client.inferenceSetupStatus() == nil)
  let profile = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"
  let selection = Components.Schemas.InferenceSetupSelection(
    answerProfile: profile, embedding: true, runtimeSource: .localArchive)
  let plan = try await client.inferenceSetupPlan(selection)
  #expect(plan.selection == selection)
  #expect(plan.canStart && plan.runtimeRequired && plan.embeddingInstalled == false)
  #expect(plan.peakAdditionalBytes == 512 && plan.maximumTransferBytes == 256)
  let id = try #require(UUID(uuidString: "e87f440f-05e4-46b9-899e-e01f47b08011"))
  await #expect(throws: AnswerRejection.self) {
    _ = try await client.startInferenceSetup(
      selection: selection, fingerprint: String(repeating: "b", count: 64), operationID: id,
      archivePath: "/fixture/pinned.zip")
  }
  await #expect(throws: AnswerRejection.self) {
    _ = try await client.startInferenceSetup(
      selection: selection, fingerprint: plan.fingerprint, operationID: id, archivePath: nil)
  }
  let started = try await client.startInferenceSetup(
    selection: plan.selection, fingerprint: plan.fingerprint, operationID: id,
    archivePath: "/fixture/pinned.zip")
  #expect(started.phase == .runtime && !started.answerReady)
  #expect(started.selection == selection)
  let completed = try #require(try await client.inferenceSetupStatus())
  #expect(completed.operationId == started.operationId)
  #expect(completed.phase == .completed && completed.answerReady)
  #expect(completed.selection == selection)
}

@MainActor
@Test(
  .enabled(if: ProcessInfo.processInfo.environment["CTXFAB_TEST_SETUP_FIXTURE_EXECUTABLE"] != nil))
func guidedSetupConfirmationProducesTheFirstCitedAnswerOnTheSameRustSession() async throws {
  let executable = try #require(
    ProcessInfo.processInfo.environment["CTXFAB_TEST_SETUP_FIXTURE_EXECUTABLE"])
  let root = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
  try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
  defer { try? FileManager.default.removeItem(at: root) }

  let fixture = Process()
  fixture.executableURL = URL(filePath: executable)
  fixture.arguments = ["--ignored", "--exact", "serve_swift_setup_fixture"]
  fixture.environment = ["CTXFAB_TEST_SETUP_FIXTURE_ROOT": root.path]
  fixture.standardOutput = FileHandle.nullDevice
  fixture.standardError = FileHandle.nullDevice
  try fixture.run()
  defer {
    if fixture.isRunning {
      fixture.terminate()
      fixture.waitUntilExit()
    }
  }

  let portFile = root.appending(path: "port")
  try await waitForSetup { FileManager.default.fileExists(atPath: portFile.path) }
  let port = try String(contentsOf: portFile, encoding: .utf8)
  let url = try #require(URL(string: "http://127.0.0.1:\(port)"))
  let client = SessionClient(url: url, token: "fixture-token")
  try await verifyGuidedSetup(using: client)
}

@MainActor
private func verifyGuidedSetup(using client: SessionClient) async throws {
  let profile = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"
  let selection = Components.Schemas.InferenceSetupSelection(
    answerProfile: profile, embedding: false, runtimeSource: .hosted)
  let store = Store(initialState: InferenceSetup.State()) {
    InferenceSetup(
      client: .init(
        plan: { try await client.inferenceSetupPlan($0) },
        start: {
          try await client.startInferenceSetup(
            selection: $0, fingerprint: $1, operationID: $2, archivePath: $3)
        },
        status: { try await client.inferenceSetupStatus() }))
  } withDependencies: {
    $0.continuousClock = ContinuousClock()
    $0.uuid = .incrementing
  }
  defer { store.send(.stop) }
  store.send(.observe)
  store.send(.review(selection, nil))
  try await waitForSetup { store.plan != nil }
  let reviewed = try #require(store.plan)
  #expect(reviewed.selection == selection)
  #expect(reviewed.canStart && reviewed.runtimeRequired && !reviewed.answerInstalled)
  #expect(reviewed.maximumTransferBytes == 8)
  await #expect(throws: AnswerRejection.self) {
    _ = try await client.startInferenceSetup(
      selection: reviewed.selection, fingerprint: String(repeating: "0", count: 64),
      operationID: UUID(), archivePath: nil)
  }
  #expect(try await client.inferenceSetupStatus() == nil)

  store.send(.confirm)
  try await waitForSetup {
    guard let status = store.status else { return false }
    return status.phase == .completed || status.phase == .failed || status.phase == .interrupted
  }
  let completed = try #require(store.status)
  try #require(completed.phase == .completed && completed.answerReady)
  #expect(completed.selection == reviewed.selection)
  #expect(try await client.inferenceSetupStatus() == completed)

  let answer = try await client.ask("What is the calibration interval?")
  let quote = "The calibration interval is exactly 37 seconds."
  #expect(answer.outcome == .answered && answer.mode == .bm25 && answer.model == profile)
  #expect(answer.text == quote && answer.citations.count == 1)
  let citation = try #require(answer.citations.first)
  #expect(citation.quote == quote)
  #expect(citation.evidence.source == "src/calibration.rs")
  #expect(citation.evidence.revision == "setup-fixture-r1")
}

@MainActor
private func waitForSetup(until ready: () -> Bool) async throws {
  let deadline = ContinuousClock.now + .seconds(10)
  while !ready() && ContinuousClock.now < deadline {
    try await Task.sleep(for: .milliseconds(25))
  }
  try #require(ready(), "Timed out waiting for the guided setup fixture")
}
