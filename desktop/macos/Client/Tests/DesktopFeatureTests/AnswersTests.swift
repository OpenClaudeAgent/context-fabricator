import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Suite(.timeLimit(.minutes(1)))
struct AnswersTests {
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
  func cancellationWaitsForTheAnswerToEndAndTargetsTheActiveRequestOnce() async {
    let (pendingAnswer, completion) = AsyncStream<Void>.makeStream()
    let (pendingReplacement, replacementCompletion) = AsyncStream<Void>.makeStream()
    defer {
      completion.finish()
      replacementCompletion.finish()
    }
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
    await store.send(.ask) { $0.answerRequest = .init(id: UUID(0)) }
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
      $0.answerRequest = .init(id: UUID(1))
      $0.answerFeedback = nil
    }
    await store.send(.answered(UUID(0), .init(answer: replacement, traceID: "obsolete")))
    await store.send(.cancelFailed(UUID(0), "Late cancellation error"))
    replacementCompletion.finish()
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = replacement
      $0.answerTraceID = "0123456789abcdef0123456789abcdef"
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

    await store.send(.ask) { $0.answerRequest = .init(id: UUID(0)) }
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
    await store.send(.ask) { $0.answerRequest = .init(id: UUID(2)) }
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
    await store.send(.ask) { $0.answerRequest = .init(id: UUID(1)) }
    await store.receive(\.answered) {
      $0.answerRequest = nil
      $0.answer = answer
    }
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
