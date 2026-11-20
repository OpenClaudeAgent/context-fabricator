import Foundation

extension SessionClient {
  public func diagnosticLogs(
    query: String, level: Components.Schemas.DiagnosticLogLevel?, beforeId: Int64?
  ) async throws -> Components.Schemas.DiagnosticLogPage {
    let parameters = Operations.readTelemetryLogs.Input.Query(
      query: query.isEmpty ? nil : query, level: level, beforeId: beforeId)
    switch try await client.readTelemetryLogs(.init(query: parameters)) {
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
