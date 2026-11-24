import LocalAPI

/// Selects only catalog entries that the backend declares compatible with setup.
public struct InferenceSetupChoices {
  public let answers: [Components.Schemas.ModelDownload]
  public let embeddings: [Components.Schemas.ModelDownload]

  public init(profiles: [Components.Schemas.ModelDownload]) {
    answers = profiles.filter {
      $0.kind == .generation
        && ($0.setupRole == .recommendedAnswer || $0.setupRole == .alternativeAnswer)
    }.sorted {
      ($0.setupRole == .recommendedAnswer ? 0 : 1) < ($1.setupRole == .recommendedAnswer ? 0 : 1)
    }
    embeddings = profiles.filter { $0.kind == .embedding && $0.setupRole == .optionalEmbedding }
  }

  public var defaultAnswer: String? { answers.first?.profile }

  public func contains(answer: String, embedding: String?) -> Bool {
    answers.contains { $0.profile == answer }
      && (embedding == nil || embeddings.contains { $0.profile == embedding })
  }
}
