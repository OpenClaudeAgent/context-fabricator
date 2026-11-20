import Foundation

extension SessionClient {
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
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }
}
