import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func actionsSelectionSendsExactObservedAttemptAndScope() async {
  let run = Components.Schemas.GitHubActionRun(
    id: 99, attempt: 2, workflowId: 20, event: "push", headSha: String(repeating: "a", count: 40),
    headBranch: "main", headRepositoryId: 41, commitProvenance: .verifiedRepositoryCommit,
    createdAt: .now, updatedAt: .now, status: "completed", conclusion: "failure")
  let snapshot = Components.Schemas.GitHubActions(
    repositoryId: 41, repository: "fixture/alpha", branch: "main",
    definitionCommit: String(repeating: "a", count: 40),
    workflowCoverage: .providerDefaultBranchList, executedDefinition: .unknown,
    runCoverage: .returnedBranchRuns, attemptCoverage: .observedLatestAttemptsOnly,
    workflows: [], runs: [run])
  let selection = ActionsSelection(repositoryId: 41, branch: "main", runId: 99, attempt: 2)
  let result = Components.Schemas.LocalAnswer(
    outcome: .insufficientEvidence, model: "q4", mode: .selected, citations: [])
  var state = Answers.State()
  state.actionsSnapshot = snapshot
  state.historySelection = .init(
    repositoryId: 41, branch: "main", child: String(repeating: "b", count: 40))
  state.question = "What happened?"
  state.models = .init(activeProfile: "q4", profiles: [.init(id: "q4", label: "Q4")])
  let store = TestStore(initialState: state) {
    Answers(
      client: .init(askActions: { question, _, actual in
        #expect(question == "What happened?")
        #expect(actual == selection)
        return .init(answer: result)
      }))
  } withDependencies: {
    $0.uuid = .incrementing
  }
  await store.send(.selectActionRun(99, 1))
  await store.send(.selectActionRun(99, 2)) {
    $0.actionsSelection = selection
    $0.historySelection = nil
  }
  await store.send(.ask) { $0.answerRequest = .init(id: UUID(0), question: "What happened?") }
  await store.receive(\.answered) {
    $0.answerRequest = nil
    $0.answer = result
  }
  await store.send(.repositoriesLoaded([])) {
    $0.actionsSelection = nil
    $0.actionsSnapshot = nil
  }
}
