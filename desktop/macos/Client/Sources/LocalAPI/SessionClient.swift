import Foundation
import HTTPTypes
import OpenAPIRuntime
import OpenAPIURLSession

public struct SessionClient: Sendable {
  let client: Client
  let longRequestClient: Client

  public init(url: URL, token: String) {
    self.client = Self.makeClient(url: url, token: token, timeout: 3)
    self.longRequestClient = Self.makeClient(url: url, token: token, timeout: 90)
  }

  private static func makeClient(url: URL, token: String, timeout: TimeInterval) -> Client {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.connectionProxyDictionary = [:]
    configuration.timeoutIntervalForRequest = timeout
    let session = URLSession(
      configuration: configuration, delegate: LocalSessionDelegate(), delegateQueue: nil)
    return Client(
      serverURL: url,
      transport: URLSessionTransport(configuration: .init(session: session)),
      middlewares: [LocalAuthorization(token: token)]
    )
  }

  public func cancelAnswer(_ id: UUID) async throws {
    switch try await client.cancelAnswer(path: .init(requestId: id.uuidString)) {
    case .ok:
      return
    case .notFound(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }

  public func generationModels() async throws -> Components.Schemas.GenerationModels {
    try await client.readGenerationModels().ok.body.json
  }

  public func selectGenerationModel(_ profile: String) async throws
    -> Components.Schemas.GenerationModels
  {
    switch try await longRequestClient.selectGenerationModel(
      .init(body: .json(.init(profile: profile))))
    {
    case .ok(let response):
      return try response.body.json
    case .notFound(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }

  public func embeddingModels() async throws -> Components.Schemas.EmbeddingModelSelection {
    switch try await client.listEmbeddingModels() {
    case .ok(let response): return try response.body.json
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func selectEmbeddingModel(_ profile: String) async throws
    -> Components.Schemas.EmbeddingModelSelection
  {
    switch try await longRequestClient.selectEmbeddingModel(
      .init(body: .json(.init(profile: profile))))
    {
    case .ok(let response): return try response.body.json
    case .badRequest(let response): throw AnswerRejection(message: try response.body.json.message)
    case .notFound(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func status() async throws -> Components.Schemas.Status {
    try await client.readStatus().ok.body.json
  }

  public func settings() async throws -> Components.Schemas.Settings {
    try await client.readSettings().ok.body.json
  }

  public func save(_ settings: Components.Schemas.Settings) async throws
    -> Components.Schemas.Settings
  {
    switch try await client.saveSettings(.init(body: .json(settings))) {
    case .ok(let response):
      return try response.body.json
    case .badRequest(let response):
      throw SettingsRejection(message: try response.body.json.message)
    case .conflict(let response):
      throw SettingsRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }

  public func telemetry() async throws -> Components.Schemas.TelemetrySnapshot {
    try await client.readTelemetry().ok.body.json
  }

  public func controlTelemetry(_ command: Components.Schemas.TelemetryControl.commandPayload)
    async throws -> Components.Schemas.TelemetrySnapshot
  {
    try await client.controlTelemetry(.init(body: .json(.init(command: command)))).ok.body.json
  }

  public func diagnostics() async throws -> [Components.Schemas.Diagnostic] {
    try await client.readDiagnostics().ok.body.json
  }
}

public struct SettingsRejection: Error, Equatable, Sendable {
  public let message: String
}

public struct AnswerRejection: LocalizedError, Equatable, Sendable {
  public let message: String
  public let traceID: String?
  public let problemCode: String?

  init(message: String, traceID: String? = nil, problemCode: String? = nil) {
    self.message = message
    self.traceID = traceID
    self.problemCode = problemCode
  }

  public var errorDescription: String? { message }
}

private struct LocalAuthorization: ClientMiddleware {
  let token: String

  func intercept(
    _ request: HTTPRequest,
    body: HTTPBody?,
    baseURL: URL,
    operationID: String,
    next: @Sendable (HTTPRequest, HTTPBody?, URL) async throws -> (HTTPResponse, HTTPBody?)
  ) async throws -> (HTTPResponse, HTTPBody?) {
    var request = request
    request.headerFields[.authorization] = "Bearer \(token)"
    request.headerFields[.userAgent] = "ContextFabricator"
    return try await next(request, body, baseURL)
  }
}

private final class LocalSessionDelegate: NSObject, URLSessionTaskDelegate {
  func urlSession(
    _ session: URLSession, task: URLSessionTask,
    willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest,
    completionHandler: @escaping @Sendable (URLRequest?) -> Void
  ) {
    completionHandler(nil)
  }
}
