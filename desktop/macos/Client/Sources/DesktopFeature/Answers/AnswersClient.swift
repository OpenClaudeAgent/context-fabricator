import Foundation
import LocalAPI

public struct CodeSelection: Equatable, Sendable {
  public let repositoryId: Int64
  public let branch: String

  public init(repositoryId: Int64, branch: String) {
    self.repositoryId = repositoryId
    self.branch = branch
  }

  func matches(_ item: Components.Schemas.GitHubConfiguredRepository) -> Bool {
    item.repository.id == repositoryId && item.branch == branch
  }
}

public struct HistorySelection: Equatable, Sendable {
  public let repositoryId: Int64
  public let branch: String
  public let child: String
  public init(repositoryId: Int64, branch: String, child: String) {
    self.repositoryId = repositoryId
    self.branch = branch
    self.child = child
  }
}

public struct ActionsSelection: Equatable, Sendable {
  public let repositoryId: Int64
  public let branch: String
  public let runId: Int64
  public let attempt: Int
  public init(repositoryId: Int64, branch: String, runId: Int64, attempt: Int) {
    self.repositoryId = repositoryId
    self.branch = branch
    self.runId = runId
    self.attempt = attempt
  }
}

public struct AnswersClient: Sendable {
  public typealias Ask = @Sendable (String, UUID) async throws -> AnswerResponse
  public typealias Cancel = @Sendable (UUID) async throws -> Void
  public typealias LoadModels = @Sendable () async throws -> Components.Schemas.GenerationModels
  public typealias SelectModel =
    @Sendable (String) async throws -> Components.Schemas.GenerationModels

  public typealias AskCode =
    @Sendable (String, UUID, CodeSelection) async throws -> AnswerResponse
  public typealias AskActions =
    @Sendable (String, UUID, ActionsSelection) async throws -> AnswerResponse
  public typealias LoadActions =
    @Sendable (Int64) async throws -> Components.Schemas.GitHubActionsState
  public typealias AskHistory =
    @Sendable (String, UUID, HistorySelection) async throws -> AnswerResponse
  public typealias LoadRepositories =
    @Sendable () async throws -> [Components.Schemas.GitHubConfiguredRepository]
  public typealias RefreshHistory =
    @Sendable (Int64) async throws -> Components.Schemas.GitHubHistory
  public typealias MaterializeHistory = @Sendable (HistorySelection) async throws -> Void

  let ask: Ask
  let askCode: AskCode
  let askHistory: AskHistory
  let askActions: AskActions
  let loadActions: LoadActions
  let loadRepositories: LoadRepositories
  let refreshHistory: RefreshHistory
  let materializeHistory: MaterializeHistory
  let cancel: Cancel
  let loadModels: LoadModels
  let selectModel: SelectModel

  public init(
    ask: @escaping Ask = { _, _ in throw BackendConnectionError.helperExited },
    askCode: @escaping AskCode = { _, _, _ in throw BackendConnectionError.helperExited },
    askHistory: @escaping AskHistory = { _, _, _ in throw BackendConnectionError.helperExited },
    askActions: @escaping AskActions = { _, _, _ in throw BackendConnectionError.helperExited },
    loadActions: @escaping LoadActions = { _ in throw BackendConnectionError.helperExited },
    loadRepositories: @escaping LoadRepositories = { [] },
    refreshHistory: @escaping RefreshHistory = { _ in throw BackendConnectionError.helperExited },
    materializeHistory: @escaping MaterializeHistory = { _ in
      throw BackendConnectionError.helperExited
    },
    cancel: @escaping Cancel = { _ in throw BackendConnectionError.helperExited },
    loadModels: @escaping LoadModels = { .init(profiles: []) },
    selectModel: @escaping SelectModel = { _ in throw BackendConnectionError.helperExited }
  ) {
    self.ask = ask
    self.askCode = askCode
    self.askHistory = askHistory
    self.askActions = askActions
    self.loadActions = loadActions
    self.loadRepositories = loadRepositories
    self.refreshHistory = refreshHistory
    self.materializeHistory = materializeHistory
    self.cancel = cancel
    self.loadModels = loadModels
    self.selectModel = selectModel
  }
}
