import Foundation

public struct GitHubRejection: LocalizedError, Equatable, Sendable {
  public let code: String
  public let message: String
  public var errorDescription: String? { message }

  public init(code: String, message: String) {
    self.code = code
    self.message = message
  }
}

extension SessionClient {
  public func githubState() async throws -> Components.Schemas.GitHubState {
    try await longRequestClient.readGitHubState().ok.body.json
  }

  public func authorizeGitHub(_ token: String, revision: Int) async throws
    -> Components.Schemas.GitHubState
  {
    switch try await longRequestClient.authorizeGitHub(
      body: .json(.init(revision: revision, token: token)))
    {
    case .ok(let response): return try response.body.json
    case .default(_, let response): throw try Self.githubRejection(response)
    default: throw URLError(.badServerResponse)
    }
  }

  public func disconnectGitHub(revision: Int) async throws -> Components.Schemas.GitHubState {
    switch try await longRequestClient.disconnectGitHub(query: .init(revision: revision)) {
    case .ok(let response): return try response.body.json
    case .default(_, let response): throw try Self.githubRejection(response)
    default: throw URLError(.badServerResponse)
    }
  }

  public func updateGitHubRateLimit(requestsPerMinute: Int, revision: Int) async throws
    -> Components.Schemas.GitHubState
  {
    switch try await longRequestClient.updateGitHubRateLimit(
      body: .json(.init(revision: revision, requestsPerMinute: requestsPerMinute)))
    {
    case .ok(let response): return try response.body.json
    case .default(_, let response): throw try Self.githubRejection(response)
    default: throw URLError(.badServerResponse)
    }
  }

  public func discoverGitHub(page: Int) async throws -> Components.Schemas.GitHubDiscovery {
    switch try await longRequestClient.discoverGitHubRepositories(query: .init(page: page)) {
    case .ok(let response): return try response.body.json
    case .default(_, let response): throw try Self.githubRejection(response)
    default: throw URLError(.badServerResponse)
    }
  }

  public func saveGitHub(_ selection: Components.Schemas.GitHubSelection) async throws
    -> Components.Schemas.GitHubState
  {
    switch try await longRequestClient.saveGitHubRepository(body: .json(selection)) {
    case .ok(let response): return try response.body.json
    case .default(_, let response): throw try Self.githubRejection(response)
    default: throw URLError(.badServerResponse)
    }
  }

  public func removeGitHub(id: Int64, revision: Int) async throws -> Components.Schemas.GitHubState
  {
    switch try await longRequestClient.removeGitHubRepository(
      path: .init(id: id), query: .init(revision: revision))
    {
    case .ok(let response): return try response.body.json
    case .default(_, let response): throw try Self.githubRejection(response)
    default: throw URLError(.badServerResponse)
    }
  }

  public func refreshGitHubCode(id: Int64) async throws -> Components.Schemas.GitHubState {
    switch try await longRequestClient.refreshGitHubCurrentCode(
      path: .init(id: id), query: .init(background: true))
    {
    case .accepted(let response): return try response.body.json
    case .default(_, let response): throw try Self.githubRejection(response)
    default: throw URLError(.badServerResponse)
    }
  }

  private static func githubRejection(_ response: Components.Responses.GitHubProblem) throws
    -> GitHubRejection
  {
    let problem = try response.body.json
    return GitHubRejection(code: problem.code, message: problem.message)
  }
}
