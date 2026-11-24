import ComposableArchitecture
import Foundation
import LocalAPI

public struct GitHubToken: Sendable, CustomDebugStringConvertible, CustomReflectable {
  let value: String
  public var debugDescription: String { "GitHubToken(redacted)" }
  public var customMirror: Mirror { Mirror(reflecting: debugDescription) }

  public init(_ value: String) {
    self.value = value
  }
}

public enum GitHubCommand: Sendable {
  case load
  case authorize(GitHubToken, revision: Int)
  case disconnect(revision: Int)
  case discover(page: Int)
  case branches(id: Int64, revision: Int, page: Int)
  case save(Components.Schemas.GitHubSelection)
  case verify(id: Int64, revision: Int)
  case remove(id: Int64, revision: Int)
  case refreshCode(Int64)
  case refreshHistory(Int64)
  case graph(Int64, branch: String, root: String?, symbol: String?)
  case actions(Int64, refresh: Bool)
  case setRate(requestsPerMinute: Int, revision: Int)
  case loadStorage
  case setStorage(limitGb: Int, revision: Int)
  case dataState(Int64)
  case repositorySettings(Int64)
  case purgeData(Int64)

  public func perform(using session: SessionClient) async throws -> GitHubResult {
    switch self {
    case .load:
      return .snapshot(try await session.githubState())
    case .authorize(let token, let revision):
      return .credential(try await session.authorizeGitHub(token.value, revision: revision))
    case .disconnect(let revision):
      return .credential(try await session.disconnectGitHub(revision: revision))
    case .discover(let page):
      return .page(try await session.discoverGitHub(page: page), replace: page == 1)
    case .save(let selection):
      return .saved(try await session.saveGitHub(selection))
    case .verify(let id, let revision):
      return .verified(try await session.checkGitHubAccess(id: id, revision: revision))
    case .remove(let id, let revision):
      return .removed(try await session.removeGitHub(id: id, revision: revision))
    case .branches, .refreshCode, .refreshHistory, .graph, .actions:
      return try await repositoryActivityResult(using: session)
    case .loadStorage, .setStorage, .dataState, .repositorySettings, .purgeData:
      return try await storageResult(using: session)
    case .setRate(let requestsPerMinute, let revision):
      return .snapshot(
        try await session.updateGitHubRateLimit(
          requestsPerMinute: requestsPerMinute, revision: revision))
    }
  }

  private func repositoryActivityResult(using session: SessionClient) async throws -> GitHubResult {
    switch self {
    case .branches(let id, let revision, let page):
      return .branches(try await session.githubBranches(id: id, revision: revision, page: page))
    case .refreshCode(let id):
      return .codeRefreshed(id, try await session.refreshGitHubCode(id: id))
    case .refreshHistory(let id):
      _ = try await session.refreshGitHubHistory(id: id)
      return .historyRefreshed(id)
    case .graph(let id, let branch, let root, let symbol):
      return .graph(
        try await session.githubGraph(id: id, branch: branch, root: root, symbol: symbol),
        root: root, symbol: symbol)
    case .actions(let id, let refresh):
      let state =
        try await (refresh ? session.refreshGitHubActions(id: id) : session.gitHubActions(id: id))
      return .actions(id, state)
    default: throw URLError(.badServerResponse)
    }
  }

  private func storageResult(using session: SessionClient) async throws -> GitHubResult {
    switch self {
    case .loadStorage: return .storage(try await session.githubStorage())
    case .setStorage(let limitGb, let revision):
      return .storage(try await session.updateGitHubStorage(limitGb: limitGb, revision: revision))
    case .dataState(let id): return .dataState(id, try await session.githubDataPaused(id: id))
    case .repositorySettings(let id):
      let paused = try await session.githubDataPaused(id: id)
      let actions = try? await session.gitHubActions(id: id)
      return .repositorySettings(id, paused: paused, actions: actions)
    case .purgeData(let id): return .purged(id, try await session.purgeGitHubData(id: id))
    default: throw URLError(.badServerResponse)
    }
  }
}

public enum GitHubResult: Equatable, Sendable {
  case snapshot(Components.Schemas.GitHubState)
  case credential(Components.Schemas.GitHubState)
  case page(Components.Schemas.GitHubDiscovery, replace: Bool)
  case branches(Components.Schemas.GitHubBranchPage)
  case saved(Components.Schemas.GitHubState)
  case verified(Components.Schemas.GitHubState)
  case removed(Components.Schemas.GitHubState)
  case graph(Components.Schemas.GitHubGraph, root: String?, symbol: String?)
  case actions(Int64, Components.Schemas.GitHubActionsState)
  case actionsRechecked(
    Int64, Components.Schemas.GitHubActionsState?, Components.Schemas.GitHubState?, paused: Bool?)
  case codeRefreshed(Int64, Components.Schemas.GitHubState)
  case historyRefreshed(Int64)
  case storage(Components.Schemas.GitHubStorageBudget)
  case dataState(Int64, Bool)
  case repositorySettings(Int64, paused: Bool, actions: Components.Schemas.GitHubActionsState?)
  case purged(Int64, Components.Schemas.GitHubStorageBudget)
}

@Reducer
public struct GitHubConnector {
  public enum Panel: Equatable {
    case account
    case repositoryAccount
    case picker
    case addOptions
    case editOptions
    case traffic
    case actions
    case graph
  }

  public struct Draft: Equatable, Sendable {
    public var repository: String
    public var expectedId: Int64?
    public var branch: String
    public var intervalInput: String
    public var features: Components.Schemas.GitHubFeatures
    public var intervalMinutes: Int? {
      Int(intervalInput.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    init(defaults: Components.Schemas.GitHubDefaults) {
      repository = ""
      branch = ""
      intervalInput = String(defaults.intervalMinutes)
      features = defaults.features
    }

    init(_ configured: Components.Schemas.GitHubConfiguredRepository) {
      repository = configured.repository.fullName
      expectedId = configured.repository.id
      branch = configured.branch
      intervalInput = String(configured.intervalMinutes)
      features = configured.features
    }

    func selection(revision: Int) -> Components.Schemas.GitHubSelection? {
      guard let intervalMinutes else { return nil }
      return .init(
        revision: revision, repository: repository, expectedId: expectedId,
        branch: branch.isEmpty ? nil : branch, intervalMinutes: intervalMinutes, features: features)
    }
  }

  struct DraftBaseline: Equatable {
    let revision: Int
    let configured: Draft?
  }

  public enum DraftResolution {
    case useLatest
    case keepEdits
  }

  @ObservableState
  public struct State: Equatable {
    public internal(set) var managing = false
    public internal(set) var snapshot: Components.Schemas.GitHubState?
    public internal(set) var candidates: [Components.Schemas.GitHubRepository] = []
    public internal(set) var nextPage: Int?
    public internal(set) var branchPicker = GitHubBranchPicker.State()
    public internal(set) var draft: Draft?
    var draftBaseline: DraftBaseline?
    public internal(set) var panel: Panel?
    public internal(set) var busy = false
    public internal(set) var feedback: String?
    public internal(set) var graphState: Components.Schemas.GitHubGraph?
    public internal(set) var graphRoot: String?
    public internal(set) var graphSymbol: String?
    public internal(set) var actionsRepositoryId: Int64?
    public internal(set) var actionsState: Components.Schemas.GitHubActionsState?
    public internal(set) var storage: Components.Schemas.GitHubStorageBudget?
    public internal(set) var dataStateId: Int64?
    public internal(set) var dataPaused: Bool?
    public internal(set) var collectionReadFailure: String?
    var workspaceVisible = false

    public var canSaveRefreshInterval: Bool {
      guard let minutes = draft?.intervalMinutes, minutes > 0 else { return false }
      guard let bounds = snapshot?.defaults.refreshInterval else { return true }
      return minutes >= bounds.minimumMinutes && minutes <= bounds.maximumMinutes
    }

    public var requiresDraftReview: Bool {
      guard let revision = snapshot?.revision, let draftBaseline else { return false }
      guard revision > draftBaseline.revision else { return false }
      guard let configured = draftBaseline.configured else { return true }
      return latestConfiguredRepository != configured
    }

    public var canReviewRepositoryEdits: Bool {
      panel == .addOptions || latestConfiguredRepository != nil
    }

    public var branchPickerTarget: GitHubBranchPicker.Target? {
      guard panel == .editOptions,
        let snapshot, snapshot.credential == .connected,
        let id = latestConfiguredRepository?.expectedId
      else { return nil }
      return .init(id: id, revision: snapshot.revision)
    }

    fileprivate var latestConfiguredRepository: Draft? {
      guard let id = draftBaseline?.configured?.expectedId else { return nil }
      return snapshot?.repositories.first { $0.repository.id == id }.map(Draft.init)
    }

    fileprivate mutating func discardDraft() {
      draft = nil
      draftBaseline = nil
    }

    public init() {}
  }

  public enum Action {
    case manage
    case close
    case workspaceVisibilityChanged(Bool)
    case reload
    case refreshCode(Int64)
    case refreshHistory(Int64)
    case openGraph(id: Int64, branch: String)
    case lookupGraph(root: String?, symbol: String?)
    case openActions(Int64)
    case refreshActions
    case collectionPolled(Components.Schemas.GitHubState)
    case collectionPollFailed(String)
    case authorize(GitHubToken)
    case disconnect
    case discover
    case loadMore
    case add
    case edit(Components.Schemas.GitHubConfiguredRepository)
    case select(Components.Schemas.GitHubRepository)
    case change(Draft)
    case openAccount
    case openRepositoryAccount
    case openTraffic
    case setRate(Int)
    case loadStorage
    case setStorage(Int)
    case loadDataState(Int64)
    case purgeData(Int64)
    case verifyAccess
    case chooseRepository(String)
    case openBranchPicker
    case branchPicker(GitHubBranchPicker.Action)
    case backToPicker
    case dismissPanel
    case resolveDraft(DraftResolution)
    case save
    case remove(Int64)
    case result(GitHubResult, feedback: String? = nil)
    case failed(String)
  }

  let perform: @Sendable (GitHubCommand) async throws -> GitHubResult
  private enum Cancellation { case collectionPoll }
  @Dependency(\.continuousClock) private var clock
  @Dependency(\.date.now) private var now

  public var body: some ReducerOf<Self> {
    Scope(state: \.branchPicker, action: \.branchPicker) {
      GitHubBranchPicker { [perform] target, page in
        guard
          case .branches(let result) = try await perform(
            .branches(id: target.id, revision: target.revision, page: page))
        else { throw URLError(.badServerResponse) }
        return result
      }
    }
    Reduce { state, action in
      switch action {
      case .manage:
        state.managing = true
        return start(.load, state: &state)
      case .close:
        state.managing = false
        return .cancel(id: Cancellation.collectionPoll)
      case .workspaceVisibilityChanged(let visible):
        guard state.workspaceVisible != visible else { return .none }
        state.workspaceVisible = visible
        if !visible { return .cancel(id: Cancellation.collectionPoll) }
        guard state.managing else { return .none }
        return start(.load, state: &state)
      case .reload:
        return start(.load, state: &state)
      case .refreshCode(let id):
        return start(.refreshCode(id), state: &state)
      case .refreshHistory(let id):
        guard state.panel == .editOptions, state.draft?.expectedId == id else { return .none }
        return start(.refreshHistory(id), state: &state)
      case .openGraph(let id, let branch):
        guard !state.busy else { return .none }
        state.graphState = nil
        state.graphRoot = nil
        state.graphSymbol = nil
        state.panel = .graph
        return start(.graph(id, branch: branch, root: nil, symbol: nil), state: &state)
      case .lookupGraph(let root, let symbol):
        guard state.panel == .graph, let graph = state.graphState else { return .none }
        return start(
          .graph(graph.repositoryId, branch: graph.branch, root: root, symbol: symbol),
          state: &state)
      case .openActions(let id):
        guard !state.busy else { return .none }
        state.actionsRepositoryId = id
        state.actionsState = nil
        state.panel = .actions
        return start(.actions(id, refresh: false), state: &state)
      case .refreshActions:
        if state.panel == .editOptions, let id = state.draft?.expectedId {
          return start(.actions(id, refresh: true), state: &state)
        }
        guard state.panel == .actions, let id = state.actionsRepositoryId,
          state.actionsState?.availability == .ready
        else {
          return .none
        }
        return start(.actions(id, refresh: true), state: &state)
      case .collectionPolled(let snapshot):
        guard state.workspaceVisible, state.managing, !state.busy else { return .none }
        updateSnapshot(snapshot, state: &state)
        return observeCollection(state)
      case .collectionPollFailed(let message):
        guard state.workspaceVisible, state.managing, !state.busy else { return .none }
        state.collectionReadFailure = message
        return .none
      case .authorize(let token):
        guard let revision = state.snapshot?.revision else { return .none }
        return start(.authorize(token, revision: revision), state: &state)
      case .disconnect:
        guard let revision = state.snapshot?.revision else { return .none }
        return start(.disconnect(revision: revision), state: &state)
      case .discover:
        return start(.discover(page: 1), state: &state)
      case .loadMore:
        guard let nextPage = state.nextPage else { return .none }
        return start(.discover(page: nextPage), state: &state)
      case .add:
        guard let snapshot = state.snapshot, !state.busy else { return .none }
        state.feedback = nil
        state.panel = .picker
        state.draft = Draft(defaults: snapshot.defaults)
        state.draftBaseline = .init(revision: snapshot.revision, configured: nil)
        return state.candidates.isEmpty ? start(.discover(page: 1), state: &state) : .none
      case .openAccount:
        state.feedback = nil
        state.panel = .account
        return .none
      case .openRepositoryAccount:
        state.feedback = nil
        state.panel = .repositoryAccount
        return .none
      case .openTraffic:
        state.panel = .traffic
        return start(.loadStorage, state: &state)
      case .loadStorage:
        return start(.loadStorage, state: &state)
      case .setStorage(let limitGb):
        guard let storage = state.storage, (1...1000).contains(limitGb) else { return .none }
        return start(.setStorage(limitGb: limitGb, revision: storage.revision), state: &state)
      case .loadDataState(let id):
        guard state.panel == .editOptions, state.draft?.expectedId == id else { return .none }
        state.dataStateId = id
        state.dataPaused = nil
        let hasActions = state.latestConfiguredRepository?.features.actions == true
        return start(hasActions ? .repositorySettings(id) : .dataState(id), state: &state)
      case .purgeData(let id):
        guard state.panel == .editOptions, state.draft?.expectedId == id else { return .none }
        return start(.purgeData(id), state: &state)
      case .setRate(let requestsPerMinute):
        guard let revision = state.snapshot?.revision else { return .none }
        return start(
          .setRate(requestsPerMinute: requestsPerMinute, revision: revision), state: &state)
      case .verifyAccess:
        let id = state.panel == .actions ? state.actionsRepositoryId : state.draft?.expectedId
        guard let id, let snapshot = state.snapshot, snapshot.credential == .connected,
          snapshot.repositories.contains(where: { $0.repository.id == id })
        else { return .none }
        return start(.verify(id: id, revision: snapshot.revision), state: &state)
      case .edit(let repository):
        guard let snapshot = state.snapshot,
          let latest = snapshot.repositories.first(where: {
            $0.repository.id == repository.repository.id
          })
        else { return .none }
        let draft = Draft(latest)
        state.dataStateId = nil
        state.dataPaused = nil
        state.feedback = nil
        state.panel = .editOptions
        state.draft = draft
        state.draftBaseline = .init(revision: snapshot.revision, configured: draft)
        return .none
      case .select(let repository):
        if state.draft?.expectedId != repository.id {
          state.draft?.branch = repository.defaultBranch
        }
        state.draft?.repository = repository.fullName
        state.draft?.expectedId = repository.id
        state.panel = .addOptions
        return .none
      case .chooseRepository(let name):
        if state.draft?.repository != name { state.draft?.branch = "" }
        state.draft?.repository = name
        state.draft?.expectedId = nil
        state.panel = .addOptions
        return .none
      case .backToPicker:
        state.feedback = nil
        state.panel = .picker
        return .none
      case .change(let draft):
        state.draft = draft
        state.feedback = nil
        return .none
      case .openBranchPicker:
        guard !state.busy, let target = state.branchPickerTarget else { return .none }
        return .send(.branchPicker(.open(target)))
      case .branchPicker(.select(let branch)):
        guard !state.busy, let target = state.branchPickerTarget,
          state.branchPicker.target == target,
          state.branchPicker.branches.contains(branch)
        else { return .none }
        state.draft?.branch = branch
        state.feedback = nil
        return .none
      case .branchPicker:
        return .none
      case .dismissPanel:
        guard !state.busy else { return .none }
        state.feedback = nil
        if state.panel == .repositoryAccount || state.panel == .actions || state.panel == .graph {
          state.panel = .editOptions
        } else {
          state.panel = nil
          state.discardDraft()
        }
        return .none
      case .resolveDraft(let resolution):
        resolveDraft(resolution, state: &state)
        return .none
      case .save:
        guard !state.requiresDraftReview, state.canSaveRefreshInterval, state.draftBaseline != nil,
          let snapshot = state.snapshot, snapshot.credential == .connected,
          let selection = state.draft?.selection(revision: snapshot.revision)
        else { return .none }
        return start(.save(selection), state: &state)
      case .remove(let id):
        guard let revision = state.snapshot?.revision else { return .none }
        return start(.remove(id: id, revision: revision), state: &state)
      case .result(let result, let feedback):
        state.busy = false
        state.feedback = feedback
        var refreshCollectionLinks = false
        switch result {
        case .branches:
          break
        case .codeRefreshed(let id, let snapshot):
          updateSnapshot(snapshot, state: &state)
          if state.dataStateId == id { state.dataPaused = false }
        case .historyRefreshed(let id):
          if state.dataStateId == id { state.dataPaused = false }
          state.feedback = "Code history refreshed."
          refreshCollectionLinks = true
        case .storage(let storage):
          state.storage = storage
        case .dataState(let id, let paused):
          if state.panel == .editOptions && state.dataStateId == id { state.dataPaused = paused }
        case .repositorySettings(let id, let paused, let actions):
          if state.panel == .editOptions && state.dataStateId == id {
            state.dataPaused = paused
            state.actionsRepositoryId = id
            state.actionsState = actions
          }
        case .purged(let id, let storage):
          state.storage = storage
          if state.panel == .editOptions && state.draft?.expectedId == id {
            state.dataStateId = id
            state.dataPaused = true
          }
        case .snapshot(let snapshot):
          updateSnapshot(snapshot, state: &state)
        case .graph(let graph, let root, let symbol):
          if state.panel == .graph {
            state.graphState = graph
            state.graphRoot = root
            state.graphSymbol = symbol
          }
        case .actions(let id, let actions):
          if state.panel == .actions && state.actionsRepositoryId == id {
            state.actionsState = actions
            refreshCollectionLinks = true
          } else if state.panel == .editOptions && state.draft?.expectedId == id {
            if state.dataStateId == id { state.dataPaused = false }
            state.actionsRepositoryId = id
            state.actionsState = actions
            state.feedback = "Workflow runs refreshed."
            refreshCollectionLinks = true
          }
        case .actionsRechecked(let id, let actions, let snapshot, let paused):
          if let snapshot { updateSnapshot(snapshot, state: &state) }
          if let actions, state.panel == .actions && state.actionsRepositoryId == id {
            state.actionsState = actions
          }
          if state.panel == .editOptions && state.draft?.expectedId == id {
            if let actions {
              state.actionsRepositoryId = id
              state.actionsState = actions
            }
            if let paused, state.dataStateId == id { state.dataPaused = paused }
          }
        case .credential(let snapshot):
          state.snapshot = snapshot
          state.collectionReadFailure = nil
          state.candidates = []
          state.nextPage = nil
          state.panel = state.panel == .repositoryAccount ? .editOptions : nil
          if state.panel == nil { state.discardDraft() }
        case .page(let page, let replace):
          if replace { state.candidates = [] }
          let previous = Set(state.candidates.map(\.id))
          state.candidates.append(
            contentsOf: page.repositories.filter { !previous.contains($0.id) })
          state.nextPage = page.nextPage
          if let traffic = page.traffic { state.snapshot?.traffic = traffic }
        case .saved(let snapshot):
          updateSnapshot(snapshot, state: &state)
          state.discardDraft()
          state.panel = nil
        case .verified(let snapshot):
          updateSnapshot(snapshot, state: &state)
          if state.panel == .actions, let id = state.actionsRepositoryId {
            return start(.actions(id, refresh: false), state: &state)
          }
        case .removed(let snapshot):
          updateSnapshot(snapshot, state: &state)
          state.discardDraft()
          state.panel = nil
        }
        return observeCollection(state, immediately: refreshCollectionLinks)
      case .failed(let message):
        state.busy = false
        state.feedback = message
        return observeCollection(state)
      }
    }
  }

  private func updateSnapshot(_ snapshot: Components.Schemas.GitHubState, state: inout State) {
    guard snapshot.revision >= (state.snapshot?.revision ?? 0) else { return }
    state.snapshot = snapshot
    state.collectionReadFailure = nil
    if snapshot.credential != .connected {
      state.candidates = []
      state.nextPage = nil
    }
  }

  private func observeCollection(_ state: State, immediately: Bool = false) -> EffectOf<Self> {
    guard state.workspaceVisible, state.managing, !state.busy else { return .none }
    if immediately { return readCollection(after: .zero) }
    guard state.collectionReadFailure == nil else { return .none }
    let collections = state.snapshot?.repositories.compactMap(\.codeCollection) ?? []
    if collections.contains(where: { $0.phase == .queued || $0.phase == .running }) {
      return readCollection(after: .seconds(2))
    }
    guard let next = collections.compactMap(\.nextAtMs).min() else { return .none }
    let untilDue = Double(next) / 1_000 - now.timeIntervalSince1970
    // A due job can remain idle while the scheduler finishes another repository.
    return readCollection(after: .seconds(untilDue > 0 ? untilDue + 2 : 10))
  }

  private func readCollection(after delay: Duration) -> EffectOf<Self> {
    let pollingClock = clock
    return .run { [perform] send in
      do {
        if delay > .zero { try await pollingClock.sleep(for: delay) }
        try Task.checkCancellation()
        guard case .snapshot(let snapshot) = try await perform(.load) else {
          throw URLError(.badServerResponse)
        }
        try Task.checkCancellation()
        await send(.collectionPolled(snapshot))
      } catch {
        if !Task.isCancelled { await send(.collectionPollFailed(error.localizedDescription)) }
      }
    }
    .cancellable(id: Cancellation.collectionPoll, cancelInFlight: true)
  }

  private func resolveDraft(_ resolution: DraftResolution, state: inout State) {
    guard !state.busy, state.requiresDraftReview, state.canReviewRepositoryEdits,
      let revision = state.snapshot?.revision, let draft = state.draft
    else { return }
    let latest = state.latestConfiguredRepository
    switch resolution {
    case .useLatest:
      guard let latest else { return }
      state.draft = latest
    case .keepEdits:
      if let baseline = state.draftBaseline?.configured, let latest {
        state.draft = draft.applyingChanges(from: baseline, to: latest)
      }
    }
    state.draftBaseline = .init(revision: revision, configured: latest)
    state.feedback = nil
  }

  private func start(_ command: GitHubCommand, state: inout State) -> EffectOf<Self> {
    guard !state.busy else { return .none }
    state.busy = true
    state.feedback = nil
    let readPausedOnFailure = state.panel == .editOptions
    return .merge(
      .cancel(id: Cancellation.collectionPoll),
      .run { [perform] send in
        do {
          await send(.result(try await perform(command)))
        } catch let rejection as GitHubRejection {
          if case .actions(let id, true) = command {
            let latest = await reloadActions(id, perform, readPaused: readPausedOnFailure)
            await send(.result(latest, feedback: rejection.message))
          } else if case .refreshHistory(let id) = command,
            let latest = try? await perform(.dataState(id))
          {
            await send(.result(latest, feedback: rejection.message))
          } else if rejection.code == "github_revision_conflict", case .setStorage = command,
            let latest = try? await perform(.loadStorage)
          {
            await send(.result(latest, feedback: rejection.message))
          } else if [
            "github_reconnect", "github_revision_conflict", "github_throttled",
            "github_access_not_verified", "github_unavailable",
          ].contains(rejection.code),
            let latest = try? await perform(.load)
          {
            await send(.result(latest, feedback: rejection.message))
          } else {
            await send(.failed(rejection.message))
          }
        } catch {
          if case .actions(let id, true) = command {
            let latest = await reloadActions(id, perform, readPaused: readPausedOnFailure)
            await send(.result(latest, feedback: error.localizedDescription))
          } else if case .refreshHistory(let id) = command,
            let latest = try? await perform(.dataState(id))
          {
            await send(.result(latest, feedback: error.localizedDescription))
          } else {
            await send(.failed(error.localizedDescription))
          }
        }
      })
  }
}

private func reloadActions(
  _ id: Int64, _ perform: @Sendable (GitHubCommand) async throws -> GitHubResult, readPaused: Bool
) async -> GitHubResult {
  let actions: Components.Schemas.GitHubActionsState? =
    if case .actions(let checkedID, let latest) = try? await perform(.actions(id, refresh: false)),
      checkedID == id
    { latest } else { nil }
  let snapshot: Components.Schemas.GitHubState? =
    if case .snapshot(let latest) = try? await perform(.load) { latest } else { nil }
  let paused: Bool? =
    if readPaused, case .dataState(let checkedID, let paused) = try? await perform(.dataState(id)),
      checkedID == id
    { paused } else { nil }
  return .actionsRechecked(id, actions, snapshot, paused: paused)
}
