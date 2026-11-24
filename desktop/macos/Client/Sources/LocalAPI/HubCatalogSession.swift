import Foundation

public typealias HubModelKind = Components.Schemas.HubModelKind

extension SessionClient {
  public func searchHubCatalog(
    _ query: String,
    kind: HubModelKind = .generation
  ) async throws -> [Components.Schemas.HubModelRepository] {
    switch try await longRequestClient.searchModelCatalog(
      .init(query: .init(query: query, kind: kind)))
    {
    case .ok(let response):
      return try response.body.json
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .notFound(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .tooManyRequests(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }

  public func hubCatalogVariants(
    _ repository: Components.Schemas.HubModelRepository, kind: HubModelKind = .generation
  ) async throws -> [Components.Schemas.HubModelVariant] {
    switch try await longRequestClient.readModelCatalogVariants(
      .init(
        query: .init(
          repository: repository.repository, revision: repository.revision, kind: kind))
    ) {
    case .ok(let response):
      return try response.body.json
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .notFound(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .tooManyRequests(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }

  public func installHubModel(
    _ repository: Components.Schemas.HubModelRepository,
    variant: Components.Schemas.HubModelVariant, kind: HubModelKind = .generation
  ) async throws -> String {
    switch try await longRequestClient.requestHubModelDownload(
      .init(
        body: .json(
          .init(
            repository: repository.repository, revision: repository.revision, kind: kind,
            sha256: variant.sha256, file: variant.file)
        ))
    ) {
    case .accepted(let response):
      return try response.body.json.profile
    case .badRequest(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .conflict(let response):
      let message = try response.body.json.message
      throw AnswerRejection(message: "Search again and review current files. \(message)")
    case .notFound(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .tooManyRequests(let response):
      throw AnswerRejection(message: try response.body.json.message)
    case .serviceUnavailable(let response):
      throw AnswerRejection(message: try response.body.json.message)
    default:
      throw URLError(.badServerResponse)
    }
  }
}
