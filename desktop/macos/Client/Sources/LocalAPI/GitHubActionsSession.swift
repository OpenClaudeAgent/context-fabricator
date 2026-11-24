import Foundation

extension SessionClient {
  public func gitHubActions(id: Int64) async throws -> Components.Schemas.GitHubActionsState {
    switch try await longRequestClient.readGitHubActions(path: .init(id: id)) {
    case .ok(let response): return try response.body.json
    case .default(_, let response):
      let problem = try response.body.json
      throw GitHubRejection(code: problem.code, message: problem.message)
    default: throw URLError(.badServerResponse)
    }
  }

  public func refreshGitHubActions(id: Int64) async throws -> Components.Schemas.GitHubActionsState
  {
    switch try await longRequestClient.refreshGitHubActions(path: .init(id: id)) {
    case .ok: return try await gitHubActions(id: id)
    case .default(_, let response):
      let problem = try response.body.json
      throw GitHubRejection(code: problem.code, message: problem.message)
    default: throw URLError(.badServerResponse)
    }
  }
}
