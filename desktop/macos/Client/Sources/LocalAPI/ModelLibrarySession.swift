import Foundation

public struct ModelRemovalConflict: LocalizedError, Sendable {
  public var errorDescription: String? { "The model changed. Review the updated removal plan." }
  public init() {}
}

extension SessionClient {
  public func discardModelPartial(_ profile: String) async throws {
    switch try await client.discardModelPartial(path: .init(profile: profile)) {
    case .noContent: return
    case .notFound(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func pauseModel(_ profile: String) async throws {
    switch try await client.pauseModelDownload(path: .init(profile: profile)) {
    case .accepted: return
    case .notFound(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func resumeModel(_ profile: String) async throws {
    switch try await client.resumeModelDownload(path: .init(profile: profile)) {
    case .accepted: return
    case .notFound(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func modelRemovalPlan(_ profile: String) async throws
    -> Components.Schemas.ModelRemovalPlan
  {
    switch try await client.readModelRemovalPlan(path: .init(profile: profile)) {
    case .ok(let response): return try response.body.json
    case .notFound(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func deleteModel(_ profile: String, identity: String? = nil) async throws {
    let body: Operations.deleteModelDownload.Input.Body? = identity.map {
      .json(.init(identity: $0))
    }
    switch try await client.deleteModelDownload(.init(path: .init(profile: profile), body: body)) {
    case .noContent:
      return
    case .notFound(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response):
      let failure = try response.body.json
      if identity != nil && failure.code == "stale_model_removal" { throw ModelRemovalConflict() }
      throw AnswerRejection(message: failure.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }

  public func modelDownloads() async throws -> [Components.Schemas.ModelDownload] {
    switch try await client.readModelDownloads() {
    case .ok(let response):
      return try response.body.json
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }

  public func installModel(_ profile: String) async throws {
    switch try await client.requestModelDownload(.init(body: .json(.init(profile: profile)))) {
    case .accepted:
      return
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .notFound(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }
}

extension SessionClient {
  public func embeddingIndex() async throws -> Components.Schemas.EmbeddingIndexState {
    switch try await client.readEmbeddingIndex() {
    case .ok(let response): return try response.body.json
    default: throw URLError(.badServerResponse)
    }
  }

  public func setEmbeddingIndexEnabled(_ enabled: Bool) async throws
    -> Components.Schemas.EmbeddingIndexState
  {
    switch try await client.setEmbeddingIndexEnabled(body: .json(.init(enabled: enabled))) {
    case .ok(let response): return try response.body.json
    default: throw URLError(.badServerResponse)
    }
  }

  public func deleteEmbeddingIndex() async throws -> Components.Schemas.EmbeddingIndexDeletion {
    switch try await client.deleteEmbeddingIndex() {
    case .ok(let response): return try response.body.json
    default: throw URLError(.badServerResponse)
    }
  }

  public func inferenceStoragePlan() async throws -> Components.Schemas.InferenceStoragePlan {
    switch try await client.getInferenceStorage() {
    case .ok(let response): return try response.body.json
    default: throw URLError(.badServerResponse)
    }
  }

  public func runtimeStatus() async throws -> Components.Schemas.RuntimeStatus {
    switch try await client.readRuntimeStatus() {
    case .ok(let response): return try response.body.json
    default: throw URLError(.badServerResponse)
    }
  }

  public func downloadRuntime() async throws -> Components.Schemas.RuntimeStatus {
    switch try await client.downloadRuntime() {
    case .ok(let response): return try response.body.json
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func installRuntime(archivePath: String) async throws -> Components.Schemas.RuntimeStatus {
    switch try await client.installRuntime(body: .json(.init(archivePath: archivePath))) {
    case .ok(let response): return try response.body.json
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }
}

extension SessionClient {
  public func inferenceSetupPlan(_ selection: Components.Schemas.InferenceSetupSelection)
    async throws -> Components.Schemas.InferenceSetupPlan
  {
    switch try await client.getInferenceSetupPlan(body: .json(selection)) {
    case .ok(let response): return try response.body.json
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func startInferenceSetup(
    selection: Components.Schemas.InferenceSetupSelection, fingerprint: String, operationID: UUID,
    archivePath: String?
  ) async throws -> Components.Schemas.InferenceSetupStatus {
    switch try await client.startInferenceSetup(
      body: .json(
        .init(
          selection: selection, fingerprint: fingerprint, operationId: operationID.uuidString,
          archivePath: archivePath)))
    {
    case .accepted(let response): return try response.body.json
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func resumeInferenceSetup(operationID: String, archivePath: String) async throws
    -> Components.Schemas.InferenceSetupStatus
  {
    switch try await client.startInferenceSetupArchive(
      body: .json(.init(operationId: operationID, archivePath: archivePath)))
    {
    case .accepted(let response): return try response.body.json
    case .badRequest(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func cancelInferenceSetupArchive(operationID: String) async throws
    -> Components.Schemas.InferenceSetupStatus
  {
    switch try await client.cancelInferenceSetupArchive(path: .init(operationId: operationID)) {
    case .ok(let response): return try response.body.json
    case .badRequest(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func inferenceSetupStatus() async throws -> Components.Schemas.InferenceSetupStatus? {
    switch try await client.getInferenceSetupStatus() {
    case .ok(let response): return try response.body.json
    case .notFound: return nil
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }
}
