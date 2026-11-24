import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func generatedClientDiscoversAndDownloadsModelWeightsThroughTheRustBackend() async throws {
  try await withHubBackend { client, _ in
    try await verifyLocalMaintenance(client)
    try await verifyCatalog(client)
  }
}

@MainActor
@Test
func importedModelFailureSurvivesCatalogRefreshAndLibraryRetryInstallsWeights() async throws {
  try await withHubBackend(retryFixture: true) { client, port in
    let repository = try #require(try await client.searchHubCatalog("catalog").first)
    let variant = try #require(
      try await client.hubCatalogVariants(repository).first { $0.quantization == .Q8_0 })
    let profile = try await client.installHubModel(repository, variant: variant)
    let store = Store(initialState: ModelLibrary.State()) {
      ModelLibrary(
        client: .init(
          catalog: { try await client.modelDownloads() },
          install: { try await client.installModel($0) }))
    } withDependencies: {
      $0.continuousClock = ContinuousClock()
    }
    defer { store.send(.stopObserving) }
    store.send(.observe)
    let failed = try await observeModel(profile, in: store) { $0.state == .failed }
    #expect(failed.source?.repository == repository.repository)
    #expect(failed.error == .model_transfer_failed)
    #expect(failed.downloadedBytes == 0 && failed.installationStage == nil)
    #expect(failed.operationId != nil)

    store.send(.stopObserving)
    #expect(store.profiles == nil)
    store.send(.observe)
    #expect(try await observeModel(profile, in: store) { $0.state == .failed } == failed)

    try await releaseFixture("allow-q8-retry", port: port)
    store.send(.install(profile))
    let active = try await observeModel(profile, in: store) { $0.downloadedBytes == 5 }
    #expect(active.state == .installing && active.installationStage == .transferring)
    #expect(active.operationId == failed.operationId && active.error == nil)
    #expect(active.totalBytes == variant.bytes)
    try await releaseFixture("release-q8", port: port)
    let installed = try await observeModel(profile, in: store) { $0.state == .installed }
    #expect(installed.downloadedBytes == variant.bytes)
    #expect(installed.error == nil && installed.installationStage == nil)
    #expect(installed.operationId == nil && store.pendingProfile == nil)
  }
}

@MainActor
private func observeModel(
  _ profile: String, in store: StoreOf<ModelLibrary>,
  when matches: (Components.Schemas.ModelDownload) -> Bool
) async throws -> Components.Schemas.ModelDownload {
  let deadline = ContinuousClock.now + .seconds(12)
  while ContinuousClock.now < deadline {
    if let model = store.profiles?.first(where: { $0.profile == profile }), matches(model) {
      return model
    }
    try await Task.sleep(for: .milliseconds(25))
  }
  try #require(Bool(false), "Timed out waiting for the model library state")
  throw URLError(.timedOut)
}

private func releaseFixture(_ action: String, port: String) async throws {
  let url = try #require(URL(string: "http://127.0.0.1:\(port)/__test/\(action)"))
  var request = URLRequest(url: url)
  request.httpMethod = "POST"
  let (_, response) = try await URLSession.shared.data(for: request)
  try #require((response as? HTTPURLResponse)?.statusCode == 204)
}

@MainActor
private func withHubBackend(
  retryFixture: Bool = false, _ run: (SessionClient, String) async throws -> Void
) async throws {
  let executable = try #require(
    ProcessInfo.processInfo.environment["CTXFAB_TEST_BACKEND_EXECUTABLE"])
  let testRoot = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
  try FileManager.default.createDirectory(at: testRoot, withIntermediateDirectories: false)
  defer { try? FileManager.default.removeItem(at: testRoot) }

  let hub = try startHubFixture(in: testRoot, retryFixture: retryFixture)
  defer { stop(hub) }
  let port = try String(
    contentsOf: try await fixtureFile("port", in: testRoot),
    encoding: .utf8)
  let directory = testRoot.appending(path: "backend")
  let backend = Process()
  backend.executableURL = URL(filePath: executable)
  backend.arguments = [directory.path]
  backend.environment = ["CTXFAB_TEST_HUGGING_FACE_PORT": port]
  backend.standardOutput = FileHandle.nullDevice
  backend.standardError = FileHandle.nullDevice
  try backend.run()
  defer { stop(backend) }

  let endpointPath = try await fixtureFile("endpoint.json", in: directory)
  let endpoint = try JSONDecoder().decode(Endpoint.self, from: Data(contentsOf: endpointPath))
  let client = SessionClient(url: endpoint.url, token: endpoint.token)
  try await run(client, port)
}

private func verifyLocalMaintenance(_ client: SessionClient) async throws {
  let storage = try await client.inferenceStoragePlan()
  #expect(storage.capacity == nil && storage.models.isEmpty)
  let initial = try await client.embeddingIndex()
  #expect(initial.enabled)
  let disabled = try await client.setEmbeddingIndexEnabled(false)
  #expect(!disabled.enabled && disabled.epoch > initial.epoch)
  let deleted = try await client.deleteEmbeddingIndex()
  #expect(deleted.value2.removedVectors == 0 && deleted.value2.removedBytes == 0)
  let after = try await client.embeddingIndex()
  #expect(!after.enabled && after.epoch > disabled.epoch)
  #expect(try await client.setEmbeddingIndexEnabled(true).enabled)
  #expect(try await client.status().outboundAttempts == 0)
}

private func verifyCatalog(_ client: SessionClient) async throws {
  #expect(try await client.searchHubCatalog("no-fake-model-matches").isEmpty)
  await #expect(throws: AnswerRejection.self) {
    _ = try await client.searchHubCatalog(String(repeating: "x", count: 81))
  }
  let repositories = try await client.searchHubCatalog("catalog")
  let repository = try #require(repositories.first)
  #expect(repository.repository == "fixture/catalog-qwen")
  #expect(repository.revision == String(repeating: "a", count: 40))
  let files = try await client.hubCatalogVariants(repository)
  let file = try #require(files.first)
  #expect(file.file == "Qwen3-4B-Instruct-2507-Q4_K_M.gguf")
  #expect(file.quantization == .Q4_K_M && file.bytes > 4)
  await #expect(throws: AnswerRejection.self) {
    _ = try await client.installHubModel(
      repository, variant: .init(file: "not-a-model.zip", quantization: .Q4_K_M, bytes: 42))
  }
  let profile = try await client.installHubModel(repository, variant: file)
  let model = try await waitForInstallation(profile, using: client)
  #expect(model.downloadedBytes == file.bytes)
  #expect(model.source?.revision == repository.revision)
  #expect(model.source?.url == repository.url)
  let plan = try await client.modelRemovalPlan(profile)
  #expect(plan.profile == profile && plan.bytes == file.bytes)
  await #expect(throws: ModelRemovalConflict.self) {
    try await client.deleteModel(profile, identity: "stale-fixture-identity")
  }
  await #expect(throws: AnswerRejection.self) {
    try await client.deleteModel(profile, identity: "")
  }
  try await client.deleteModel(profile, identity: plan.identity)
  await #expect(throws: AnswerRejection.self) {
    _ = try await client.modelRemovalPlan(profile)
  }
  let removed = try await client.modelDownloads().first { $0.profile == profile }
  #expect(removed?.state == .notInstalled)
  #expect(try await client.status().outboundAttempts == 7)
}

private func waitForInstallation(_ profile: String, using client: SessionClient) async throws
  -> Components.Schemas.ModelDownload
{
  let deadline = ContinuousClock.now + .seconds(8)
  while ContinuousClock.now < deadline {
    if let model = try await client.modelDownloads().first(where: { $0.profile == profile }),
      model.state == .installed
    {
      return model
    }
    try await Task.sleep(for: .milliseconds(50))
  }
  try #require(Bool(false), "The selected model did not finish downloading")
  throw URLError(.timedOut)
}

private func startHubFixture(in directory: URL, retryFixture: Bool) throws -> Process {
  let script = URL(filePath: #filePath)
    .deletingLastPathComponent()
    .appending(path: "../../../../../quality/models/hugging_face_fixture.py").standardizedFileURL
  let process = Process()
  process.executableURL = URL(filePath: "/usr/bin/env")
  process.arguments =
    ["python3", script.path, directory.path] + (retryFixture ? ["--download-retry"] : [])
  process.standardOutput = FileHandle.nullDevice
  process.standardError = FileHandle.nullDevice
  try process.run()
  return process
}

private func fixtureFile(_ name: String, in directory: URL) async throws -> URL {
  let file = directory.appending(path: name)
  let deadline = ContinuousClock.now + .seconds(5)
  while !FileManager.default.fileExists(atPath: file.path), ContinuousClock.now < deadline {
    try await Task.sleep(for: .milliseconds(25))
  }
  try #require(FileManager.default.fileExists(atPath: file.path), "Missing fixture file \(name)")
  return file
}

private func stop(_ process: Process) {
  if process.isRunning {
    process.terminate()
    process.waitUntilExit()
  }
}

private struct Endpoint: Decodable {
  let url: URL
  let token: String
}
