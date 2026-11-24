import Foundation

extension SessionClient {
  public func answerUsage() async throws -> Components.Schemas.AnswerUsage {
    try await client.readAnswerUsage().ok.body.json
  }
}
