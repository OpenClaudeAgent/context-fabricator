import Foundation

extension SessionClient {
  public func answerHistorySettings() async throws -> Components.Schemas.AnswerHistorySettings {
    switch try await client.getAnswerHistorySettings(.init()) {
    case .ok(let response): return try response.body.json
    default: throw URLError(.badServerResponse)
    }
  }
  public func saveAnswerHistorySettings(_ value: Components.Schemas.AnswerHistorySettings)
    async throws -> Components.Schemas.AnswerHistorySettings
  {
    switch try await client.updateAnswerHistorySettings(.init(body: .json(value))) {
    case .ok(let response): return try response.body.json
    case .badRequest(let response): throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response): throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }
  public func answerHistory(beforeId: Int64?) async throws -> Components.Schemas.AnswerHistoryPage {
    switch try await client.listAnswerHistory(.init(query: .init(beforeId: beforeId))) {
    case .ok(let response): return try response.body.json
    default: throw URLError(.badServerResponse)
    }
  }
  public func answerHistoryEntry(id: Int64) async throws -> Components.Schemas.AnswerHistoryEntry {
    switch try await client.getAnswerHistoryEntry(.init(path: .init(id: id))) {
    case .ok(let response): return try response.body.json
    case .notFound(let response): throw AnswerRejection(message: try response.body.json.message)
    default: throw URLError(.badServerResponse)
    }
  }
  public func answerHistoryEntry(traceId: String) async throws -> Components.Schemas
    .AnswerHistoryEntry?
  {
    switch try await client.getAnswerHistoryEntryByTrace(.init(path: .init(traceId: traceId))) {
    case .ok(let response): return try response.body.json
    case .notFound: return nil
    default: throw URLError(.badServerResponse)
    }
  }
  public func clearAnswerHistory() async throws {
    switch try await client.clearAnswerHistory(.init()) {
    case .noContent: return
    default: throw URLError(.badServerResponse)
    }
  }
}
