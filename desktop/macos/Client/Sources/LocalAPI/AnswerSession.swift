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

  public func askWithTrace(_ query: String, id: UUID? = nil) async throws -> AnswerResponse {
    switch try await longRequestClient.askQuestion(
      .init(body: .json(.init(query: query, requestId: id?.uuidString)))
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
      throw AnswerRejection(
        message: try response.body.json.message,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id)
    case .serviceUnavailable(let response):
      throw AnswerRejection(
        message: try response.body.json.message,
        traceID: response.headers.X_hyphen_Context_hyphen_Trace_hyphen_Id)
    default:
      throw URLError(.badServerResponse)
    }
  }
}
