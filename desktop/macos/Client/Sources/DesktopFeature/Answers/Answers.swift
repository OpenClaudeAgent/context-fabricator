import ComposableArchitecture
import Foundation
import LocalAPI

public struct AnswersClient: Sendable {
  public typealias Ask = @Sendable (String, UUID) async throws -> AnswerResponse
  public typealias Cancel = @Sendable (UUID) async throws -> Void
  public typealias LoadModels = @Sendable () async throws -> Components.Schemas.GenerationModels
  public typealias SelectModel =
    @Sendable (String) async throws -> Components.Schemas.GenerationModels

  let ask: Ask
  let cancel: Cancel
  let loadModels: LoadModels
  let selectModel: SelectModel

  public init(
    ask: @escaping Ask = { _, _ in throw BackendConnectionError.helperExited },
    cancel: @escaping Cancel = { _ in throw BackendConnectionError.helperExited },
    loadModels: @escaping LoadModels = { .init(profiles: []) },
    selectModel: @escaping SelectModel = { _ in throw BackendConnectionError.helperExited }
  ) {
    self.ask = ask
    self.cancel = cancel
    self.loadModels = loadModels
    self.selectModel = selectModel
  }
}

@Reducer
public struct Answers {
  struct AnswerRequest: Equatable {
    let id: UUID
    var cancelling = false
  }

  struct ModelRequest: Equatable {
    let id: UUID
    let profile: String?
  }

  @ObservableState
  public struct State: Equatable {
    public internal(set) var question = ""
    public internal(set) var answer: Components.Schemas.LocalAnswer?
    public internal(set) var answerFeedback: String?
    public internal(set) var answerTraceID: String?
    public internal(set) var models: Components.Schemas.GenerationModels?
    public internal(set) var modelFeedback: String?
    var answerRequest: AnswerRequest?
    var modelRequest: ModelRequest?

    public var answerBusy: Bool { answerRequest != nil }
    public var answerCancelling: Bool { answerRequest?.cancelling == true }
    public var loadingModel: String? { modelRequest?.profile }
    public var modelSwitching: Bool { loadingModel != nil }

    public var activeModel: Components.Schemas.GenerationProfile? {
      models?.profiles.first { $0.id == models?.activeProfile }
    }

    public var canAsk: Bool {
      !answerBusy && !modelSwitching && activeModel != nil
        && !question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
  }

  public enum Action {
    case questionChanged(String)
    case ask
    case answered(UUID, AnswerResponse)
    case answerFailed(UUID, String, traceID: String?)
    case cancelAnswer
    case cancelFailed(UUID, String)
    case loadModels
    case selectModel(String)
    case modelsLoaded(UUID, Components.Schemas.GenerationModels)
    case modelControlFailed(UUID, String)
  }

  private enum Cancellation: Hashable { case models }

  private let client: AnswersClient
  @Dependency(\.uuid) private var uuid

  public init(client: AnswersClient) { self.client = client }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .questionChanged(let question):
        state.question = question
        return .none
      case .ask:
        return ask(&state)
      case .answered(let id, let response):
        guard state.answerRequest?.id == id else { return .none }
        state.answerRequest = nil
        state.answer = response.answer
        state.answerTraceID = response.traceID
        state.answerFeedback = nil
        return .none
      case .answerFailed(let id, let message, let traceID):
        guard state.answerRequest?.id == id else { return .none }
        state.answerRequest = nil
        state.answerFeedback = message
        state.answerTraceID = traceID
        return .none
      case .cancelAnswer:
        return cancelAnswer(&state)
      case .cancelFailed(let id, let message):
        guard state.answerRequest?.id == id, state.answerCancelling else { return .none }
        state.answerRequest?.cancelling = false
        state.answerFeedback = message
        return .none
      case .loadModels:
        guard !state.modelSwitching else { return .none }
        return requestModels(&state, profile: nil, operation: client.loadModels)
      case .selectModel(let profile):
        guard !state.answerBusy, !state.modelSwitching, state.models?.activeProfile != profile
        else { return .none }
        state.answerFeedback = nil
        return requestModels(&state, profile: profile) { [client] in
          try await client.selectModel(profile)
        }
      case .modelsLoaded(let id, let models):
        guard state.modelRequest?.id == id else { return .none }
        state.modelRequest = nil
        state.models = models
        return .none
      case .modelControlFailed(let id, let message):
        guard state.modelRequest?.id == id else { return .none }
        state.modelRequest = nil
        state.modelFeedback = message
        return .none
      }
    }
  }

  private func ask(_ state: inout State) -> EffectOf<Self> {
    guard state.canAsk else { return .none }
    let id = uuid()
    state.answerRequest = .init(id: id)
    state.answer = nil
    state.answerFeedback = nil
    state.answerTraceID = nil
    return .run { [client, question = state.question] send in
      do {
        await send(.answered(id, try await client.ask(question, id)))
      } catch {
        await send(
          .answerFailed(
            id, error.localizedDescription, traceID: (error as? AnswerRejection)?.traceID))
      }
    }
  }

  private func cancelAnswer(_ state: inout State) -> EffectOf<Self> {
    guard let id = state.answerRequest?.id, !state.answerCancelling else { return .none }
    state.answerRequest?.cancelling = true
    state.answerFeedback = nil
    return .run { [client] send in
      do {
        try await client.cancel(id)
      } catch {
        await send(.cancelFailed(id, error.localizedDescription))
      }
    }
  }

  private func requestModels(
    _ state: inout State, profile: String?, operation: @escaping AnswersClient.LoadModels
  ) -> EffectOf<Self> {
    let id = uuid()
    state.modelRequest = .init(id: id, profile: profile)
    state.modelFeedback = nil
    return .run { send in
      do {
        let models = try await operation()
        try Task.checkCancellation()
        await send(.modelsLoaded(id, models))
      } catch {
        if !Task.isCancelled { await send(.modelControlFailed(id, error.localizedDescription)) }
      }
    }
    .cancellable(id: Cancellation.models, cancelInFlight: true)
  }
}
