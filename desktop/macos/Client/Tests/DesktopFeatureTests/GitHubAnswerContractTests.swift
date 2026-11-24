import Foundation
import LocalAPI
import Testing

@Test
func generatedClientTracesACitedActionsAnswerAndPurgesItsHistory() async throws {
  try await withGitHubBackendFixture(answerWorker: true) { session, origin in
    let repository = try await collectActions(using: session, origin: origin)
    var history = try await session.answerHistorySettings()
    history.enabled = true
    history.retentionDays = 7
    #expect(try await session.saveAnswerHistorySettings(history).enabled)

    let entry = try await verifyCitedAnswer(using: session, repository: repository)
    try await verifyUnobservedAttempt(using: session, repository: repository)

    _ = try await session.purgeGitHubData(id: repository)
    #expect(try await session.githubDataPaused(id: repository))
    #expect(try await session.answerHistory(beforeId: nil).entries.isEmpty)
    let traceID = try #require(entry.traceId)
    #expect(try await session.answerHistoryEntry(traceId: traceID) == nil)
  }
}

private func collectActions(using session: SessionClient, origin: URL) async throws -> Int64 {
  for flag in ["actions-fixture", "actions-completed"] {
    try Data().write(to: origin.appending(path: flag))
  }
  let initial = try await session.githubState()
  let connected = try await session.authorizeGitHub(
    "github_pat_fixture_primary", revision: initial.revision)
  let discovered = try await session.discoverGitHub(page: 1)
  let repository = try #require(discovered.repositories.first { $0.fullName == "fixture/alpha" })
  let configured = try await session.saveGitHub(
    .init(
      revision: connected.revision, repository: repository.fullName, expectedId: repository.id,
      branch: "main", intervalMinutes: connected.defaults.intervalMinutes,
      features: .init(currentCode: false, history: false, actions: true, codeGraph: false)))
  #expect(configured.repositories.first?.actions == .available)
  let collected = try await session.refreshGitHubActions(id: repository.id)
  let run = try #require(collected.snapshot?.runs.first)
  #expect(run.id == 30 && run.attempt == 1 && run.conclusion == "success")
  return repository.id
}

private func verifyCitedAnswer(using session: SessionClient, repository: Int64) async throws
  -> Components.Schemas.AnswerHistoryEntry
{
  let question = "What was the conclusion?"
  let response = try await session.askWithTrace(
    question, repositoryId: repository, branch: "main", actionsRunId: 30, actionsRunAttempt: 1)
  #expect(response.answer.outcome == .answered)
  #expect(response.answer.text == "Conclusion: success")
  #expect(response.answer.evidenceRepositoryIds == [repository])
  #expect(response.answer.citations.count == 1)
  let citation = try #require(response.answer.citations.first)
  #expect(citation.quote == "Conclusion: success" && citation.lineUrl == nil)
  let workflow = try #require(citation.workflowRun)
  #expect(workflow.runId == 30 && workflow.attempt == 1 && workflow.repositoryId == repository)
  #expect(workflow.runUrl == "https://github.com/fixture/alpha/actions/runs/30/attempts/1")
  #expect(workflow.reportedHead == String(repeating: "a", count: 40))
  let context = try #require(response.answer.modelContext)
  #expect(context.question == question)
  #expect(context.sources.first?.kind == .workflow)
  let passage = try #require(context.sources.first?.parts.first?.text)
  #expect(passage.components(separatedBy: "\n").contains("Conclusion: success"))

  let traceID = try #require(response.traceID)
  try await verifyRetainedTrace(traceID, for: question, using: session)
  let entry = try #require(try await session.answerHistoryEntry(traceId: traceID))
  #expect(entry.question == question && entry.traceId == traceID)
  #expect(entry.answer.modelContext == nil && entry.answer.citations == [citation])
  #expect(entry.scope.actionsRunId == 30 && entry.scope.actionsRunAttempt == 1)
  return entry
}

private func verifyUnobservedAttempt(using session: SessionClient, repository: Int64) async throws {
  let response = try await session.askWithTrace(
    "What was the conclusion of the second attempt?",
    repositoryId: repository, branch: "main", actionsRunId: 30, actionsRunAttempt: 2)
  #expect(response.answer.outcome == .insufficientEvidence)
  #expect(response.answer.text == nil && response.answer.citations.isEmpty)
  let review = try #require(response.answer.evidenceReview)
  #expect(review.reason == .noCandidates && review.candidates.isEmpty)
  #expect(
    review.searches
      == [.init(source: .githubActions, candidateCount: 0, selection: "Run 30 · attempt 2")])
}

private func verifyRetainedTrace(_ id: String, for question: String, using session: SessionClient)
  async throws
{
  let deadline = ContinuousClock.now + .seconds(3)
  repeat {
    let trace = try await session.diagnosticTrace(id, beforeId: nil)
    if let generation = trace.spans.first(where: { $0.name == "answer_generation" }),
      let root = trace.spans.first(where: { $0.name == "api_answer" })
    {
      #expect(root.traceId == id && root.spanOutcome == .succeeded)
      #expect(root.httpStatus == 200 && generation.parentSpanId == root.spanId)
      #expect(generation.contextBudget?.kind == .workflowAttempt)
      #expect(generation.contextBudget?.admitted == 1)
      let payload = try JSONEncoder().encode(trace)
      #expect(!String(decoding: payload, as: UTF8.self).contains(question))
      #expect(!String(decoding: payload, as: UTF8.self).contains("Conclusion: success"))
      return
    }
    try await Task.sleep(for: .milliseconds(30))
  } while ContinuousClock.now < deadline
  Issue.record("The answer's correlated root and generation spans were not retained.")
}
