import ComposableArchitecture
import Foundation
import LocalAPI

@Reducer
public struct Answers {
  struct AnswerRequest: Equatable {
    let id: UUID
    let question: String
    var cancelling = false
    var purgedRepositories: Set<Int64> = []
    var keepsSessionQuestion = true
  }

  public struct SessionQuestion: Identifiable, Equatable, Sendable {
    public let id: String
    public let text: String
    public let outcome: Components.Schemas.LocalAnswer.outcomePayload?
  }

  struct ModelRequest: Equatable {
    let id: UUID
    let profile: String?
  }

  @ObservableState
  public struct State: Equatable {
    public internal(set) var codeRepositories: [Components.Schemas.GitHubConfiguredRepository] = []
    public internal(set) var codeSelection: CodeSelection?
    public internal(set) var repositoryFeedback: String?
    public internal(set) var actionsRepositories: [Components.Schemas.GitHubConfiguredRepository] =
      []
    public internal(set) var actionsRepositoryId: Int64?
    public internal(set) var actionsSnapshot: Components.Schemas.GitHubActions?
    public internal(set) var actionsSelection: ActionsSelection?
    public internal(set) var actionsBusy = false
    public internal(set) var actionsFeedback: String?
    public internal(set) var historyRepositories: [Components.Schemas.GitHubConfiguredRepository] =
      []
    public internal(set) var historyRepositoryId: Int64?
    public internal(set) var historyLineage: Components.Schemas.GitHubHistory?
    public internal(set) var historySelection: HistorySelection?
    public internal(set) var historyBusy = false
    public internal(set) var historyFeedback: String?
    public internal(set) var question = ""
    public internal(set) var answer: Components.Schemas.LocalAnswer?
    public internal(set) var answerFeedback: String?
    public internal(set) var answerTraceID: String?
    public internal(set) var sessionQuestions: [SessionQuestion] = []
    public internal(set) var models: Components.Schemas.GenerationModels?
    public internal(set) var modelFeedback: String?
    var answerRequest: AnswerRequest?
    var modelRequest: ModelRequest?
    var scopeNeedsSelection = false

    public var answerBusy: Bool { answerRequest != nil }
    public var answerCancelling: Bool { answerRequest?.cancelling == true }
    public var loadingModel: String? { modelRequest?.profile }
    public var modelSwitching: Bool { loadingModel != nil }

    public var selectedCodeRepository: Components.Schemas.GitHubConfiguredRepository? {
      guard let codeSelection else { return nil }
      return codeRepositories.first(where: codeSelection.matches)
    }

    public var scopeUnavailable: Bool {
      scopeNeedsSelection || (codeSelection != nil && selectedCodeRepository == nil)
    }

    public var activeModel: Components.Schemas.GenerationProfile? {
      models?.profiles.first { $0.id == models?.activeProfile }
    }

    public var canAsk: Bool {
      !answerBusy && !historyBusy && !modelSwitching && !scopeUnavailable && activeModel != nil
        && !question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    mutating func discardSessionQuestions() {
      sessionQuestions.removeAll()
      answerRequest?.keepsSessionQuestion = false
    }

    fileprivate mutating func remember(
      _ request: AnswerRequest, traceID: String?,
      outcome: Components.Schemas.LocalAnswer.outcomePayload?
    ) {
      guard request.keepsSessionQuestion, let traceID, request.question.unicodeScalars.count <= 512
      else { return }
      sessionQuestions.removeAll { $0.id == traceID }
      sessionQuestions.insert(.init(id: traceID, text: request.question, outcome: outcome), at: 0)
      if sessionQuestions.count > 20 { sessionQuestions.removeLast() }
    }
  }

  public enum Action {
    case loadActions(Int64)
    case actionsLoaded(Int64, Components.Schemas.GitHubActionsState)
    case selectActionRun(Int64, Int)
    case clearActions
    case actionsFailed(Int64, String)
    case loadRepositories
    case repositoriesLoaded([Components.Schemas.GitHubConfiguredRepository])
    case repositoriesFailed(String)
    case selectCode(CodeSelection?)
    case repositoryDataPurged(Int64)
    case refreshHistory(Int64)
    case historyLoaded(Int64, Components.Schemas.GitHubHistory)
    case prepareHistory(String)
    case historyPrepared(HistorySelection)
    case clearHistory
    case historyFailed(String)
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

  private enum Cancellation: Hashable { case models, repositories, history, actions }

  private let client: AnswersClient
  @Dependency(\.uuid) private var uuid

  public init(client: AnswersClient) { self.client = client }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .loadActions(let id):
        guard !state.actionsBusy, !state.historyBusy, !state.answerBusy else { return .none }
        state.actionsBusy = true
        state.actionsRepositoryId = id
        state.actionsSnapshot = nil
        state.actionsSelection = nil
        state.actionsFeedback = nil
        return .run { [client] send in
          do {
            let result = try await client.loadActions(id)
            try Task.checkCancellation()
            await send(.actionsLoaded(id, result))
          } catch {
            if !Task.isCancelled { await send(.actionsFailed(id, error.localizedDescription)) }
          }
        }
        .cancellable(id: Cancellation.actions, cancelInFlight: true)
      case .actionsLoaded(let id, let result):
        guard state.actionsRepositoryId == id else { return .none }
        state.actionsBusy = false
        if result.availability != .ready {
          state.actionsFeedback = "Enable Actions and verify read access in Connectors."
        } else if let snapshot = result.snapshot, snapshot.repositoryId == id,
          state.actionsRepositories.contains(where: {
            $0.repository.id == id && $0.branch == snapshot.branch
          })
        {
          state.actionsSnapshot = snapshot
        } else {
          state.actionsFeedback = "No Actions data for this branch. Refresh Actions in Connectors."
        }
        return .none
      case .selectActionRun(let id, let attempt):
        guard !state.answerBusy, !state.historyBusy, let snapshot = state.actionsSnapshot,
          snapshot.runs.contains(where: { $0.id == id && $0.attempt == attempt })
        else { return .none }
        state.historySelection = nil
        state.codeSelection = nil
        state.scopeNeedsSelection = false
        state.actionsSelection = .init(
          repositoryId: snapshot.repositoryId, branch: snapshot.branch, runId: id, attempt: attempt)
        return .none
      case .clearActions:
        state.actionsSelection = nil
        return .none
      case .actionsFailed(let id, let message):
        guard state.actionsRepositoryId == id else { return .none }
        state.actionsBusy = false
        state.actionsFeedback = message
        return .none
      case .loadRepositories:
        state.repositoryFeedback = nil
        return .run { [client] send in
          do {
            let repositories = try await client.loadRepositories()
            try Task.checkCancellation()
            await send(.repositoriesLoaded(repositories))
          } catch {
            if !Task.isCancelled { await send(.repositoriesFailed(error.localizedDescription)) }
          }
        }
        .cancellable(id: Cancellation.repositories, cancelInFlight: true)
      case .repositoriesLoaded(let repositories):
        state.repositoryFeedback = nil
        state.codeRepositories = repositories.filter { $0.features.currentCode }
        state.historyRepositories = repositories.filter { $0.features.history }
        state.actionsRepositories = repositories.filter { $0.features.actions }
        if let snapshot = state.actionsSnapshot,
          !state.actionsRepositories.contains(where: {
            $0.repository.id == snapshot.repositoryId && $0.branch == snapshot.branch
          })
        {
          state.actionsSelection = nil
          state.actionsSnapshot = nil
          state.actionsRepositoryId = nil
        }
        return .none
      case .repositoriesFailed(let message):
        state.repositoryFeedback = message
        return .none
      case .repositoryDataPurged(let id):
        return repositoryDataPurged(id, state: &state)
      case .selectCode(let selection):
        guard !state.answerBusy, !state.historyBusy, !state.actionsBusy else { return .none }
        if let selection, !state.codeRepositories.contains(where: selection.matches) {
          return .none
        }
        guard
          state.scopeNeedsSelection || state.codeSelection != selection
            || state.historySelection != nil
            || state.actionsSelection != nil
        else { return .none }
        state.codeSelection = selection
        state.scopeNeedsSelection = false
        state.historySelection = nil
        state.actionsSelection = nil
        state.answer = nil
        state.answerFeedback = nil
        state.answerTraceID = nil
        return .none
      case .refreshHistory(let id):
        guard !state.historyBusy, !state.actionsBusy, !state.answerBusy else { return .none }
        state.historyBusy = true
        state.historyRepositoryId = id
        state.historyLineage = nil
        state.historySelection = nil
        state.historyFeedback = nil
        return .run { [client] send in
          do {
            let lineage = try await client.refreshHistory(id)
            try Task.checkCancellation()
            await send(.historyLoaded(id, lineage))
          } catch {
            if !Task.isCancelled { await send(.historyFailed(error.localizedDescription)) }
          }
        }
        .cancellable(id: Cancellation.history, cancelInFlight: true)
      case .historyLoaded(let id, let lineage):
        guard state.historyRepositoryId == id else { return .none }
        state.historyBusy = false
        state.historyLineage = lineage
        return .none
      case .prepareHistory(let child):
        guard !state.historyBusy, !state.actionsBusy, !state.answerBusy,
          let id = state.historyRepositoryId,
          let repository = state.historyRepositories.first(where: { $0.repository.id == id }),
          state.historyLineage?.commits.contains(where: { $0.id == child && !$0.parents.isEmpty })
            == true
        else { return .none }
        let selection = HistorySelection(repositoryId: id, branch: repository.branch, child: child)
        state.historyBusy = true
        state.historyFeedback = nil
        return .run { [client] send in
          do {
            try await client.materializeHistory(selection)
            try Task.checkCancellation()
            await send(.historyPrepared(selection))
          } catch {
            if !Task.isCancelled { await send(.historyFailed(error.localizedDescription)) }
          }
        }
        .cancellable(id: Cancellation.history, cancelInFlight: true)
      case .historyPrepared(let selection):
        guard state.historyBusy, state.historyRepositoryId == selection.repositoryId else {
          return .none
        }
        state.historyBusy = false
        state.historySelection = selection
        state.actionsSelection = nil
        state.codeSelection = nil
        state.scopeNeedsSelection = false
        return .none
      case .clearHistory:
        state.historySelection = nil
        return .none
      case .historyFailed(let message):
        state.historyBusy = false
        state.historyFeedback = message
        return .none
      case .questionChanged(let question):
        state.question = question
        return .none
      case .ask:
        return ask(&state)
      case .answered(let id, let response):
        guard let request = state.answerRequest, request.id == id else { return .none }
        state.answerRequest = nil
        if affected(response.answer, by: request.purgedRepositories) {
          clearPurgedAnswer(&state)
          return .none
        }
        state.answer = response.answer
        state.answerTraceID = response.traceID
        state.answerFeedback = nil
        state.remember(request, traceID: response.traceID, outcome: response.answer.outcome)
        return .none
      case .answerFailed(let id, let message, let traceID):
        guard let request = state.answerRequest, request.id == id else { return .none }
        state.answerRequest = nil
        state.answerFeedback = message
        state.answerTraceID = traceID
        state.remember(request, traceID: traceID, outcome: nil)
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

  private func repositoryDataPurged(_ id: Int64, state: inout State) -> EffectOf<Self> {
    state.answerRequest?.purgedRepositories.insert(id)
    state.discardSessionQuestions()
    if let answer = state.answer, affected(answer, by: [id]) { clearPurgedAnswer(&state) }

    if state.historySelection?.repositoryId == id {
      state.historySelection = nil
      state.scopeNeedsSelection = true
    }
    if state.actionsSelection?.repositoryId == id {
      state.actionsSelection = nil
      state.scopeNeedsSelection = true
    }

    var effects: [EffectOf<Self>] = []
    if state.historyRepositoryId == id {
      state.historyRepositoryId = nil
      state.historyLineage = nil
      state.historyBusy = false
      state.historyFeedback = nil
      effects.append(.cancel(id: Cancellation.history))
    }
    if state.actionsRepositoryId == id {
      state.actionsRepositoryId = nil
      state.actionsSnapshot = nil
      state.actionsBusy = false
      state.actionsFeedback = nil
      effects.append(.cancel(id: Cancellation.actions))
    }
    return .merge(effects)
  }

  private func affected(_ answer: Components.Schemas.LocalAnswer, by ids: Set<Int64>) -> Bool {
    guard !ids.isEmpty else { return false }
    if answer.evidenceReview != nil { return true }
    guard answer.outcome == .answered else { return false }
    return answer.evidenceRepositoryIds?.contains(where: ids.contains) ?? true
  }

  private func clearPurgedAnswer(_ state: inout State) {
    state.answer = nil
    state.answerTraceID = nil
    state.answerFeedback = "Local repository data used by this answer was deleted."
  }

  private func ask(_ state: inout State) -> EffectOf<Self> {
    guard state.canAsk else { return .none }
    let id = uuid()
    state.answerRequest = .init(id: id, question: state.question)
    state.answer = nil
    state.answerFeedback = nil
    state.answerTraceID = nil
    return .run {
      [
        client, question = state.question, history = state.historySelection,
        actions = state.actionsSelection, code = state.codeSelection
      ] send in
      do {
        let response: AnswerResponse
        if let actions {
          response = try await client.askActions(question, id, actions)
        } else if let history {
          response = try await client.askHistory(question, id, history)
        } else if let code {
          response = try await client.askCode(question, id, code)
        } else {
          response = try await client.ask(question, id)
        }
        await send(.answered(id, response))
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
