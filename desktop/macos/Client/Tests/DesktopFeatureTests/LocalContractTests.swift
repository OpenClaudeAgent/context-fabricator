import Foundation
import LocalAPI
import Testing

@Test
func generatedClientReadsAndUpdatesTheLiveRustService() async throws {
  let executable = try #require(
    ProcessInfo.processInfo.environment["CTXFAB_TEST_BACKEND_EXECUTABLE"])
  let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
  let process = Process()
  process.executableURL = URL(filePath: executable)
  process.arguments = [directory.path]
  process.standardOutput = FileHandle.nullDevice
  process.standardError = FileHandle.nullDevice
  try process.run()
  defer {
    if process.isRunning {
      process.terminate()
      process.waitUntilExit()
    }
    try? FileManager.default.removeItem(at: directory)
  }

  let endpointPath = directory.appending(path: "endpoint.json")
  let deadline = ContinuousClock.now + .seconds(5)
  while !FileManager.default.fileExists(atPath: endpointPath.path), ContinuousClock.now < deadline {
    try await Task.sleep(for: .milliseconds(30))
  }
  let endpoint = try JSONDecoder().decode(Endpoint.self, from: Data(contentsOf: endpointPath))
  let client = SessionClient(url: endpoint.url, token: endpoint.token)
  let status = try await client.status()
  #expect(status.status == .ready)
  #expect(status.connectedSources == 0)
  #expect(status.collectedDocuments == 0)
  #expect(status.indexedDocuments == 0)
  #expect(try await client.diagnostics().contains { $0.code == "backend_started" })
  try await verifyDiagnosticSettings(client)
  #expect(try await client.controlTelemetry(.enableDebug).health.debugUntil > 0)
  #expect(try await client.controlTelemetry(.disableDebug).health.debugUntil == 0)
  try await verifyAnswerErrors(client)
  try await verifyGenerationControls(client)
  try await verifyModelAndLogCatalogs(client)
}

private func verifyModelAndLogCatalogs(_ client: SessionClient) async throws {
  let catalog = try await client.modelDownloads()
  #expect(catalog.count == 3)
  #expect(catalog.allSatisfy { $0.label != nil && $0.state == .notInstalled })
  #expect(catalog.filter { $0.kind == .generation }.count == 2)
  #expect(catalog.filter { $0.kind == .embedding }.count == 1)
  do {
    try await client.installModel("unlisted-fixture-profile")
    Issue.record("The backend accepted an unknown model.")
  } catch let failure as AnswerRejection {
    #expect(failure.message == "This model profile is not available for download.")
  }
  let results = try await client.diagnosticLogs(
    query: "nonexistent-stage", level: .error, beforeId: nil)
  #expect(results.records.isEmpty && results.nextBeforeId == nil)
  #expect(try await client.status().outboundAttempts == 0)
}

private func verifyDiagnosticSettings(_ client: SessionClient) async throws {
  var draft = try await client.settings()
  draft.diagnosticRetentionHours = 0
  do {
    _ = try await client.save(draft)
    Issue.record("The generated client accepted invalid retention.")
  } catch let failure as SettingsRejection {
    #expect(failure.message == "Diagnostic retention must be between 1 and 168 hours.")
  }

  draft.diagnosticRetentionHours = 48
  draft.answerLanguage = .fr
  draft.diagnosticStorageLimitMb = 1
  do {
    _ = try await client.save(draft)
    Issue.record("The generated client accepted an invalid diagnostics budget.")
  } catch let failure as SettingsRejection {
    #expect(failure.message == "Diagnostic storage limit must be between 2 and 1000 MB.")
  }
  draft.diagnosticStorageLimitMb = 1000
  let updated = try await client.save(draft)
  #expect(updated.answerLanguage == .fr)
  #expect(updated.diagnosticRetentionHours == 48)
  #expect(updated.diagnosticStorageLimitMb == 1000)
  #expect(updated.revision == draft.revision + 1)
  #expect(try await client.telemetry().health.retentionHours == 48)
  #expect(try await client.telemetry().health.budgetBytes == 1_000_000_000)
}

private func verifyGenerationControls(_ client: SessionClient) async throws {
  let models = try await client.generationModels()
  #expect(models.activeProfile == nil)
  #expect(models.profiles.isEmpty)
  do {
    _ = try await client.selectGenerationModel("unprovisioned")
    Issue.record("Expected an unprovisioned profile rejection.")
  } catch let failure as AnswerRejection {
    #expect(failure.message == "This generation profile is not provisioned locally.")
  }
  do {
    try await client.cancelAnswer(UUID())
    Issue.record("Expected an inactive request rejection.")
  } catch let failure as AnswerRejection {
    #expect(failure.message == "There is no active answer with this request ID.")
  }
}

private func verifyAnswerErrors(_ client: SessionClient) async throws {
  for (question, expected) in [
    ("How are attempts paced?", "The local generation model is not configured."),
    (" ", "Search text must contain 1 to 512 characters."),
  ] {
    do {
      _ = try await client.ask(question)
      Issue.record("Expected a typed answer rejection.")
    } catch let failure as AnswerRejection {
      #expect(failure.message == expected)
      let traceID = try #require(failure.traceID)
      #expect(traceID.count == 32)
      try await verifyRejectedAnswerTrace(traceID, client: client)
    }
  }
}

private func verifyRejectedAnswerTrace(_ id: String, client: SessionClient) async throws {
  let deadline = ContinuousClock.now + .seconds(3)
  while ContinuousClock.now < deadline {
    let page = try await client.diagnosticTrace(id, beforeId: nil)
    if let span = page.spans.first {
      #expect(span.name == "api_answer" && span.level == .error && span.traceId == id)
      #expect(span.displayName == "Answer request")
      #expect(span.operation == "POST /v1/answers")
      #expect(span.events?.isEmpty == true)
      let start = try #require(span.startedAtUnixMicros)
      let duration = try #require(span.value)
      #expect(start > 0 && start / 1_000_000 <= span.timestamp)
      #expect(duration >= 0)
      #expect(page.logs.contains { $0.spanId == span.spanId && $0.level == .error })
      #expect(page.nextBeforeId == nil)
      return
    }
    try await Task.sleep(for: .milliseconds(100))
  }
  Issue.record("The rejected answer's trace was not retained.")
}

private struct Endpoint: Decodable {
  let url: URL
  let token: String
}
