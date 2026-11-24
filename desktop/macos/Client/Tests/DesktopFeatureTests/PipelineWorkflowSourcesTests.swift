import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@Test
func fourWorkflowAttemptsStayActiveAcrossRepositoriesAndIndependentDeactivation() throws {
  var sources = PipelineWorkflowSources()
  sources.load(snapshot(repository: "fixture/alpha", id: 41, branch: "main"))
  sources.load(snapshot(repository: "fixture/beta", id: 42, branch: "trunk"))
  let first = try #require(sources.runs(for: 41))
  let second = try #require(sources.runs(for: 42))
  let workflows = first + second

  for run in workflows { sources.setActive(true, for: run) }
  sources.setActive(true, for: first[0])
  #expect(sources.active.count == 4)
  #expect(Set(sources.active.map(\.id)).count == 4)
  #expect(sources.active.map(\.attempt) == [1, 2, 1, 2])
  #expect(sources.active.map(\.branch) == ["main", "main", "trunk", "trunk"])

  sources.setActive(false, for: first[0])
  #expect(!sources.isActive(first[0]))
  #expect(sources.active == [first[1]] + second)
  #expect(sources.runs(for: 41) == first)

  let saved = Set(sources.active.map(\.id))
  var restored = PipelineWorkflowSources()
  restored.load(snapshot(repository: "fixture/beta", id: 42, branch: "trunk"))
  restored.load(snapshot(repository: "fixture/alpha", id: 41, branch: "main"))
  restored.restoreSelection(saved.union(["stale/run"]))
  #expect(restored.active == [first[1]] + second)
}

private func snapshot(
  repository: String, id: Int64, branch: String
) -> Components.Schemas.GitHubActions {
  let commit = String(repeating: "a", count: 40)
  return .init(
    repositoryId: id, repository: repository, branch: branch, definitionCommit: commit,
    workflowCoverage: .providerDefaultBranchList, executedDefinition: .unknown,
    runCoverage: .returnedBranchRuns, attemptCoverage: .observedLatestAttemptsOnly,
    workflows: [],
    runs: [1, 2].map { attempt in
      .init(
        id: 30, attempt: attempt, workflowId: 20, event: "push", headSha: commit,
        headBranch: branch, headRepositoryId: id, commitProvenance: .verifiedRepositoryCommit,
        createdAt: Date(timeIntervalSince1970: 0), updatedAt: Date(timeIntervalSince1970: 0),
        status: "completed", conclusion: "success")
    })
}
