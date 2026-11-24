import LocalAPI

public struct PipelineWorkflow: Hashable, Identifiable, Sendable {
  public let repositoryID: Int64
  public let repository: String
  public let branch: String
  public let runID: Int64
  public let attempt: Int
  public let name: String
  public let commit: String

  public var id: String { "\(repositoryID)/\(branch)/\(runID)/\(attempt)" }
}

public struct PipelineWorkflowSources: Sendable {
  private var snapshots: [Int64: [PipelineWorkflow]] = [:]
  public private(set) var active: [PipelineWorkflow] = []

  public init() {}

  public var all: [PipelineWorkflow] { snapshots.values.flatMap { $0 } }

  public func runs(for repositoryID: Int64) -> [PipelineWorkflow]? {
    snapshots[repositoryID]
  }

  public mutating func restoreSelection(_ identifiers: Set<String>) {
    active = snapshots.values.flatMap { $0 }.filter { identifiers.contains($0.id) }
      .sorted {
        ($0.repository, $0.branch, $0.runID, $0.attempt) < (
          $1.repository, $1.branch, $1.runID, $1.attempt
        )
      }
  }

  public mutating func load(_ snapshot: Components.Schemas.GitHubActions) {
    snapshots[snapshot.repositoryId] = Self.workflows(snapshot)
  }

  public mutating func loadAll(_ downloads: [Components.Schemas.GitHubActions]) {
    snapshots = Dictionary(grouping: downloads.flatMap(Self.workflows), by: \.repositoryID)
      .mapValues { runs in
        Dictionary(runs.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
          .values.sorted { ($0.runID, $0.attempt) > ($1.runID, $1.attempt) }
      }
  }

  private static func workflows(_ snapshot: Components.Schemas.GitHubActions) -> [PipelineWorkflow]
  {
    let names = Dictionary(
      snapshot.workflows.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
    return snapshot.runs.map { run in
      PipelineWorkflow(
        repositoryID: snapshot.repositoryId, repository: snapshot.repository,
        branch: snapshot.branch,
        runID: run.id, attempt: run.attempt,
        name: names[run.workflowId] ?? snapshot.repository, commit: run.headSha)
    }
  }

  public func isActive(_ workflow: PipelineWorkflow) -> Bool {
    active.contains { $0.id == workflow.id }
  }

  public mutating func setActive(_ enabled: Bool, for workflow: PipelineWorkflow) {
    if enabled {
      guard !isActive(workflow) else { return }
      active.append(workflow)
    } else {
      active.removeAll { $0.id == workflow.id }
    }
  }
}
