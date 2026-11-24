import LocalAPI

extension GitHubConnector.Draft {
  func applyingChanges(from baseline: Self, to latest: Self) -> Self {
    var merged = latest
    if branch != baseline.branch { merged.branch = branch }
    if intervalMinutes != baseline.intervalMinutes { merged.intervalInput = intervalInput }
    let options: [WritableKeyPath<Components.Schemas.GitHubFeatures, Bool>] = [
      \.currentCode, \.history, \.actions, \.codeGraph,
    ]
    for option in options where features[keyPath: option] != baseline.features[keyPath: option] {
      merged.features[keyPath: option] = features[keyPath: option]
    }
    return merged
  }
}
