import Foundation

public struct AnswerResponse: Equatable, Sendable {
  public let answer: Components.Schemas.LocalAnswer
  public let traceID: String?

  public init(answer: Components.Schemas.LocalAnswer, traceID: String? = nil) {
    self.answer = answer
    self.traceID = traceID
  }
}

extension SessionClient {
  public func ask(_ query: String, id: UUID? = nil) async throws -> Components.Schemas.LocalAnswer {
    try await askWithTrace(query, id: id).answer
  }

  public func askWithTrace(
    _ query: String, id: UUID? = nil, repositoryId: Int64? = nil, branch: String? = nil,
    historyCommit: String? = nil, actionsRunId: Int64? = nil, actionsRunAttempt: Int? = nil,
    inspectPipeline: Bool? = nil
  ) async throws -> AnswerResponse {
    switch try await longRequestClient.askQuestion(
      .init(
        body: .json(
          .init(
            query: query, repositoryId: repositoryId, branch: branch, historyCommit: historyCommit,
            actionsRunId: actionsRunId, actionsRunAttempt: actionsRunAttempt,
            requestId: id?.uuidString, includeModelContext: true, inspectPipeline: inspectPipeline))
      )
    ) {
    case .ok(let response):
      return AnswerResponse(
        answer: try response.body.json,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id)
    case .badRequest(let response):
      throw AnswerRejection(
        message: try response.body.json.message,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id)
    case .conflict(let response):
      let problem = try response.body.json
      throw AnswerRejection(
        message: problem.message,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id,
        problemCode: problem.code)
    case .serviceUnavailable(let response):
      throw AnswerRejection(
        message: try response.body.json.message,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id)
    default:
      throw URLError(.badServerResponse)
    }
  }
}

extension SessionClient {
  public func askBenchmark(
    _ query: String, id: UUID, documents: [Components.Schemas.BenchmarkDocument]? = nil,
    reference: Components.Schemas.BenchmarkReference? = nil,
    action: Components.Schemas.BenchmarkAction? = nil,
    retrieval: Components.Schemas.BenchmarkQuestion.retrievalPayload? = nil
  ) async throws -> AnswerResponse {
    switch try await longRequestClient.askBenchmarkQuestion(
      .init(
        body: .json(
          .init(
            query: query, requestId: id.uuidString, reference: reference, action: action,
            retrieval: retrieval,
            documents: documents))
      )
    ) {
    case .ok(let response):
      return AnswerResponse(
        answer: try response.body.json,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id)
    case .badRequest(let response):
      throw AnswerRejection(
        message: try response.body.json.message,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id)
    case .conflict(let response):
      let problem = try response.body.json
      throw AnswerRejection(
        message: problem.message,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id,
        problemCode: problem.code)
    case .serviceUnavailable(let response):
      throw AnswerRejection(
        message: try response.body.json.message,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id)
    default:
      throw URLError(.badServerResponse)
    }
  }

  public func loadBenchmarkCorpus(
    _ reference: Components.Schemas.BenchmarkReference
  ) async throws -> Components.Schemas.BenchmarkCorpus {
    switch try await longRequestClient.loadBenchmarkCorpus(body: .json(reference)) {
    case .ok(let response): return try response.body.json
    case .badRequest(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func listBenchmarkCorpora() async throws -> [Components.Schemas.BenchmarkCorpus] {
    switch try await client.listBenchmarkCorpora() {
    case .ok(let response): return try response.body.json
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func unloadBenchmarkCorpus(_ reference: Components.Schemas.BenchmarkReference) async throws
  {
    switch try await longRequestClient.unloadBenchmarkCorpus(body: .json(reference)) {
    case .noContent: break
    case .badRequest(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func embedBenchmarkCorpusBatch(
    _ reference: Components.Schemas.BenchmarkReference
  ) async throws -> Components.Schemas.RetrievalProgress {
    switch try await longRequestClient.embedBenchmarkCorpusBatch(body: .json(reference)) {
    case .ok(let response): return try response.body.json
    case .badRequest(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response):
      let problem = try response.body.json
      throw AnswerRejection(message: problem.message, problemCode: problem.code)
    case .serviceUnavailable(let response):
      let problem = try response.body.json
      throw AnswerRejection(message: problem.message, problemCode: problem.code)
    default: throw URLError(.badServerResponse)
    }
  }

  public func recordedBenchmarkActions() async throws -> [Components.Schemas.GitHubActions] {
    switch try await longRequestClient.listRecordedBenchmarkActions() {
    case .ok(let response): return try response.body.json
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }
}

extension SessionClient {
  public func benchmarkActionDownloads() async throws
    -> [Components.Schemas.BenchmarkActionDownload]
  {
    switch try await client.listBenchmarkActionDownloads() {
    case .ok(let response): return try response.body.json
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func downloadBenchmarkActions(_ reference: Components.Schemas.BenchmarkReference)
    async throws -> Components.Schemas.BenchmarkActionDownload
  {
    switch try await longRequestClient.downloadBenchmarkActions(body: .json(reference)) {
    case .ok(let response): return try response.body.json
    case .badRequest(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }
}
