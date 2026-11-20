import Foundation

extension SessionClient {
  public func diagnosticTrace(_ id: String, beforeId: Int64?) async throws
    -> Components.Schemas.DiagnosticTracePage
  {
    switch try await client.readTrace(
      .init(path: .init(traceId: id), query: .init(beforeId: beforeId)))
    {
    case .ok(let response):
      return try response.body.json
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }
}
