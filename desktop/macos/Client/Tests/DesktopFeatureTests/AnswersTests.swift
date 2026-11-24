import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Suite(.timeLimit(.minutes(1)))
struct AnswersTests {
  @Test
  func sourcePurgeClearsTheTransientReviewForAWithheldAnswer() async {
    var initial = Answers.State()
    initial.answer = .init(
      outcome: .insufficientEvidence, model: "q4", mode: .bm25,
      evidenceReview: .init(
        question: "Local question", searches: [], candidates: [], reason: .noCandidates),
      citations: [])
    initial.answerTraceID = "fixture-trace"
    let store = TestStore(initialState: initial) { Answers(client: .init()) }
    await store.send(.repositoryDataPurged(41)) {
      $0.answer = nil
      $0.answerTraceID = nil
      $0.answerFeedback = "Local repository data used by this answer was deleted."
    }
  }

  @Test
  func askingRequiresANonemptyDraftAndAConfirmedModel() async {
    let catalog = Self.models(active: "q4")
    let store = TestStore(initialState: Answers.State()) {
      Answers(client: .init(loadModels: { catalog }))
    } withDependencies: {
      $0.uuid = .incrementing
    }

    await store.send(.questionChanged("Retry cooldown")) { $0.question = "Retry cooldown" }
    await store.send(.ask)
    await store.send(.loadModels) { $0.modelRequest = .init(id: UUID(0), profile: nil) }
    await store.receive(\.modelsLoaded) {
      $0.modelRequest = nil
      $0.models = catalog
    }
    #expect(store.state.canAsk)
    await store.send(.questionChanged(" \n")) { $0.question = " \n" }
    await store.send(.ask)
    #expect(!store.state.canAsk)
  }

  @Test
  func sessionHistoryRetainsTheSubmittedQuestionAfterEditingAndAfterAnotherAnswer() async {
    let (pending, completion) = AsyncStream<Void>.makeStream()
    defer { completion.finish() }
    let withheld = Components.Schemas.LocalAnswer(
      outcome: .insufficientEvidence, model: "q4", mode: .bm25, citations: [])
    let published = Self.groundedAnswer(model: "q4")
    let first = Answers.SessionQuestion(
      id: "withheld-trace", text: "Original question", outcome: .insufficientEvidence)
    let second = Answers.SessionQuestion(
      id: "published-trace", text: "Next question", outcome: .answered)
    let store = Self.makeStore(
      question: first.text,
      client: .init(ask: { question, id in
        if id == UUID(0) {
          #expect(question == first.text)
          for await _ in pending {}
          return .init(answer: withheld, traceID: first.id)
        }
        #expect(question == second.text)
        return .init(answer: published, traceID: second.id)
      }))

    await store.send(.ask) { $0.answerRequest = .init(id: UUID(0), question: first.text) }
    await store.send(.questionChanged(second.text)) { $0.question = second.text }
    completion.finish()
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = withheld
      $0.answerTraceID = first.id
      $0.sessionQuestions = [first]
    }
    await store.send(.ask) {
      $0.answerRequest = .init(id: UUID(1), question: second.text)
      $0.answer = nil
      $0.answerTraceID = nil
    }
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = published
      $0.answerTraceID = second.id
      $0.sessionQuestions = [second, first]
    }
  }

  @Test
  func cancellationWaitsForTheAnswerToEndAndTargetsTheActiveRequestOnce() async {
    let (pendingAnswer, completion) = AsyncStream<Void>.makeStream()
    let (pendingReplacement, replacementCompletion) = AsyncStream<Void>.makeStream()
    let cancelled = LockIsolated<[UUID]>([])
    let replacement = Self.groundedAnswer(model: "q4")
    let store = Self.makeStore(
      question: "Hold the answer",
      client: .init(
        ask: { question, id in
          if question == "Hold the answer" {
            #expect(id == UUID(0))
            for await _ in pendingAnswer {}
            throw FixtureError.cancelled
          }
          #expect(question == "Retry cooldown")
          #expect(id == UUID(1))
          for await _ in pendingReplacement {}
          return .init(answer: replacement, traceID: "0123456789abcdef0123456789abcdef")
        },
        cancel: { id in cancelled.withValue { $0.append(id) } }))

    await store.send(.cancelAnswer)
    await store.send(.ask) { $0.answerRequest = .init(id: UUID(0), question: "Hold the answer") }
    await store.send(.cancelAnswer) { $0.answerRequest?.cancelling = true }.finish()
    #expect(store.state.answerBusy && store.state.answerCancelling && store.state.answer == nil)
    await store.send(.cancelAnswer)
    await store.send(.ask)
    await store.send(.selectModel("q5"))
    #expect(cancelled.value == [UUID(0)])

    completion.finish()
    await store.receive(\.answerFailed) {
      $0.answerRequest = nil
      $0.answerFeedback = FixtureError.cancelled.localizedDescription
    }
    #expect(store.state.canAsk)
    await store.send(.questionChanged("Retry cooldown")) { $0.question = "Retry cooldown" }
    await store.send(.ask) {
      $0.answerRequest = .init(id: UUID(1), question: "Retry cooldown")
      $0.answerFeedback = nil
    }
    await store.send(.answered(UUID(0), .init(answer: replacement, traceID: "obsolete")))
    await store.send(.cancelFailed(UUID(0), "Late cancellation error"))
    replacementCompletion.finish()
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = replacement
      $0.answerTraceID = "0123456789abcdef0123456789abcdef"
      $0.sessionQuestions = [
        .init(id: "0123456789abcdef0123456789abcdef", text: "Retry cooldown", outcome: .answered)
      ]
    }
  }

  @Test
  func failedCancellationLeavesTheAnswerPendingAndAllowsARetry() async {
    let (pendingAnswer, completion) = AsyncStream<Void>.makeStream()
    defer { completion.finish() }
    let attempts = LockIsolated<[UUID]>([])
    let store = Self.makeStore(
      question: "Hold the answer",
      client: .init(
        ask: { _, _ in
          for await _ in pendingAnswer {}
          throw FixtureError.cancelled
        },
        cancel: { id in
          let first = attempts.withValue {
            $0.append(id)
            return $0.count == 1
          }
          if first { throw FixtureError.unavailable }
        }))

    await store.send(.ask) { $0.answerRequest = .init(id: UUID(0), question: "Hold the answer") }
    await store.send(.cancelAnswer) { $0.answerRequest?.cancelling = true }
    await store.receive(\.cancelFailed) {
      $0.answerRequest?.cancelling = false
      $0.answerFeedback = FixtureError.unavailable.localizedDescription
    }
    #expect(store.state.answerBusy && !store.state.canAsk)
    await store.send(.cancelAnswer) {
      $0.answerRequest?.cancelling = true
      $0.answerFeedback = nil
    }.finish()
    #expect(attempts.value == [UUID(0), UUID(0)])
    completion.finish()
    await store.receive(\.answerFailed) {
      $0.answerRequest = nil
      $0.answerFeedback = FixtureError.cancelled.localizedDescription
    }
  }

  @Test
  func confirmedSelectionWinsOverLateCatalogRepliesAndUnblocksAsking() async {
    let original = Self.models(active: "q4")
    let selected = Self.models(active: "q5")
    let (catalog, previousRead) = AsyncStream<Void>.makeStream()
    let (warmup, selection) = AsyncStream<Void>.makeStream()
    defer {
      previousRead.finish()
      selection.finish()
    }
    let reads = LockIsolated(0)
    let switches = LockIsolated<[String]>([])
    let answer = Self.groundedAnswer(model: "q5")
    let store = Self.makeStore(
      question: "Retry cooldown",
      client: .init(
        ask: { _, _ in .init(answer: answer) },
        loadModels: {
          reads.withValue { $0 += 1 }
          for await _ in catalog {}
          return original
        },
        selectModel: { profile in
          switches.withValue { $0.append(profile) }
          for await _ in warmup {}
          return selected
        }))

    await store.send(.selectModel("q4"))
    await store.send(.loadModels) { $0.modelRequest = .init(id: UUID(0), profile: nil) }
    await store.send(.selectModel("q5")) { $0.modelRequest = .init(id: UUID(1), profile: "q5") }
    #expect(store.state.activeModel?.id == "q4")
    #expect(!store.state.canAsk)
    await store.send(.ask)
    await store.send(.selectModel("q5"))
    await store.send(.loadModels)
    await store.send(.modelsLoaded(UUID(0), original))
    await store.send(.modelControlFailed(UUID(0), "Old catalog error"))
    selection.finish()
    await store.receive(\.modelsLoaded) {
      $0.modelRequest = nil
      $0.models = selected
    }
    #expect(store.state.activeModel?.id == "q5")
    #expect(store.state.canAsk)
    await store.send(.ask) { $0.answerRequest = .init(id: UUID(2), question: "Retry cooldown") }
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = answer
    }
    #expect(reads.value == 1)
    #expect(switches.value == ["q5"])
  }

  @Test
  func failedModelSwitchKeepsTheConfirmedModelAndAllowsAsking() async {
    let original = Self.models(active: "q4")
    let answer = Self.groundedAnswer(model: "q4")
    let store = Self.makeStore(
      question: "Retry cooldown",
      client: .init(
        ask: { question, _ in
          #expect(question == "Retry cooldown")
          return .init(answer: answer)
        },
        selectModel: { profile in
          #expect(profile == "q5")
          throw FixtureError.unavailable
        }))

    await store.send(.selectModel("q5")) { $0.modelRequest = .init(id: UUID(0), profile: "q5") }
    await store.receive(\.modelControlFailed) {
      $0.modelRequest = nil
      $0.modelFeedback = FixtureError.unavailable.localizedDescription
    }
    #expect(store.state.models == original)
    #expect(store.state.canAsk)
    await store.send(.ask) { $0.answerRequest = .init(id: UUID(1), question: "Retry cooldown") }
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = answer
    }
  }

  @Test
  func selectedHistoryIsPreparedAndSentWithItsExactScope() async {
    let fixture = Self.historyFixture()
    let repository = fixture.repository
    let lineage = fixture.lineage
    let child = lineage.tip
    let selection = HistorySelection(repositoryId: 41, branch: "main", child: child)
    let answer = Self.groundedAnswer(model: "q4")
    let store = Self.makeStore(
      question: "What changed?",
      client: .init(
        askHistory: { question, _, actual in
          #expect(question == "What changed?")
          #expect(actual == selection)
          return .init(answer: answer)
        },
        loadRepositories: { [repository] },
        refreshHistory: { id in
          #expect(id == 41)
          return lineage
        },
        materializeHistory: { actual in #expect(actual == selection) }))
    await store.send(.loadRepositories)
    await store.receive(\.repositoriesLoaded) { $0.historyRepositories = [repository] }
    await store.send(.refreshHistory(41)) {
      $0.historyBusy = true
      $0.historyRepositoryId = 41
    }
    await store.receive(\.historyLoaded) {
      $0.historyBusy = false
      $0.historyLineage = lineage
    }
    await store.send(.prepareHistory(child)) { $0.historyBusy = true }
    await store.receive(\.historyPrepared) {
      $0.historyBusy = false
      $0.historySelection = selection
    }
    await store.send(.ask) { $0.answerRequest = .init(id: UUID(0), question: "What changed?") }
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = answer
    }
    await store.send(.clearHistory) { $0.historySelection = nil }
  }

  private struct HistoryFixture {
    let repository: Components.Schemas.GitHubConfiguredRepository
    let lineage: Components.Schemas.GitHubHistory
  }

  @Test
  func currentCodeScopeSurvivesRefreshWithoutWideningTheQuestion() async {
    var current = Self.historyFixture().repository
    current.features.currentCode = true
    let selection = CodeSelection(repositoryId: 41, branch: "main")
    var moved = current
    moved.branch = "release"
    let answer = Self.groundedAnswer(model: "q4")
    let store = Self.makeStore(
      question: "Where is the limit?",
      client: .init(
        ask: { _, _ in
          Issue.record("An explicit repository scope was widened.")
          throw FixtureError.unavailable
        },
        askCode: { question, id, actual in
          #expect(question == "Where is the limit?")
          #expect(id == UUID(0))
          #expect(actual == selection)
          return .init(answer: answer, traceID: "scoped-answer")
        }))

    await store.send(.repositoriesLoaded([current])) {
      $0.codeRepositories = [current]
      $0.historyRepositories = [current]
    }
    await store.send(.selectCode(.init(repositoryId: 41, branch: "untracked")))
    await store.send(.selectCode(selection)) { $0.codeSelection = selection }
    await store.send(.ask) {
      $0.answerRequest = .init(id: UUID(0), question: "Where is the limit?")
    }
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = answer
      $0.answerTraceID = "scoped-answer"
      $0.sessionQuestions = [
        .init(id: "scoped-answer", text: "Where is the limit?", outcome: .answered)
      ]
    }
    await store.send(.repositoriesLoaded([moved])) {
      $0.codeRepositories = [moved]
      $0.historyRepositories = [moved]
    }
    #expect(store.state.codeSelection == selection)
    #expect(store.state.scopeUnavailable)
    #expect(!store.state.canAsk)
    await store.send(.ask)
    await store.send(.selectCode(nil)) {
      $0.codeSelection = nil
      $0.answer = nil
      $0.answerTraceID = nil
    }
    #expect(store.state.canAsk)
  }

  @Test(arguments: [true, false])
  func inFlightAnswerUsesOwnershipFromTheBackend(usesPurgedRepository: Bool) async {
    let id = UUID(0)
    var state = Answers.State()
    state.answerRequest = .init(id: id, question: "What changed?")
    var answer = Self.groundedAnswer(model: "q4")
    answer.evidenceRepositoryIds = usesPurgedRepository ? [41, 42] : [42]
    let store = TestStore(initialState: state) { Answers(client: .init()) }
    await store.send(.repositoryDataPurged(41)) {
      $0.answerRequest?.purgedRepositories = [41]
      $0.answerRequest?.keepsSessionQuestion = false
    }
    await store.send(.answered(id, .init(answer: answer, traceID: "answer-trace"))) {
      $0.answerRequest = nil
      if usesPurgedRepository {
        $0.answerFeedback = "Local repository data used by this answer was deleted."
      } else {
        $0.answer = answer
        $0.answerTraceID = "answer-trace"
      }
    }
  }

  @Test
  func purgedHistoryScopeRequiresExplicitReselection() async {
    let fixture = Self.historyFixture()
    let selection = HistorySelection(repositoryId: 41, branch: "main", child: fixture.lineage.tip)
    var state = Answers.State()
    state.models = Self.models(active: "q4")
    state.question = "What changed?"
    state.historyRepositoryId = 41
    state.historySelection = selection
    state.historyLineage = fixture.lineage
    let store = TestStore(initialState: state) { Answers(client: .init()) }

    await store.send(.repositoryDataPurged(41)) {
      $0.historySelection = nil
      $0.historyRepositoryId = nil
      $0.historyLineage = nil
      $0.scopeNeedsSelection = true
    }
    await store.send(.historyLoaded(41, fixture.lineage))
    await store.send(.historyPrepared(selection))
    #expect(store.state.scopeUnavailable)
    #expect(!store.state.canAsk)
    await store.send(.ask)
    await store.send(.selectCode(nil)) { $0.scopeNeedsSelection = false }
    #expect(store.state.canAsk)
    #expect(store.state.question == "What changed?")
  }

  private static func historyFixture() -> HistoryFixture {
    let features = Components.Schemas.GitHubFeatures(
      currentCode: false, history: true, actions: false, codeGraph: false)
    let repository = Components.Schemas.GitHubConfiguredRepository(
      repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: true),
      branch: "main", intervalMinutes: 60, features: features,
      contents: .available, actions: .unchecked, verified: true)
    let child = String(repeating: "a", count: 40)
    let lineage = Components.Schemas.GitHubHistory(
      branch: "main", tip: child, boundary: .root,
      commits: [
        .init(
          id: child, parents: [String(repeating: "d", count: 40)],
          tree: String(repeating: "b", count: 40), summary: "Start")
      ])
    return HistoryFixture(repository: repository, lineage: lineage)
  }

  private static func makeStore(question: String, client: AnswersClient) -> TestStoreOf<Answers> {
    var state = Answers.State()
    state.models = models(active: "q4")
    state.question = question
    return TestStore(initialState: state) {
      Answers(client: client)
    } withDependencies: {
      $0.uuid = .incrementing
    }
  }

  private static func models(active: String) -> Components.Schemas.GenerationModels {
    .init(
      activeProfile: active,
      profiles: [.init(id: "q4", label: "Q4_K_M"), .init(id: "q5", label: "Q5_K_M")])
  }

  private static func groundedAnswer(model: String) -> Components.Schemas.LocalAnswer {
    .init(
      outcome: .answered, model: model, mode: .hybrid, text: "47 seconds.",
      citations: [
        .init(
          evidence: .init(
            source: "src/outbound.rs", revision: "r-current", kind: .current, title: "Retry"),
          quote: "The retry cooldown is 47 seconds.")
      ])
  }

  private enum FixtureError: LocalizedError {
    case cancelled
    case unavailable

    var errorDescription: String? {
      switch self {
      case .cancelled: "The answer request was cancelled."
      case .unavailable: "The required operation is temporarily unavailable."
      }
    }
  }
}
