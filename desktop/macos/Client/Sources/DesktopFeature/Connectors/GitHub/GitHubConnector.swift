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
  case save(Components.Schemas.GitHubSelection)
  case verify(Components.Schemas.GitHubSelection)
  case remove(id: Int64, revision: Int)
  case refreshCode(Int64)
  case setRate(requestsPerMinute: Int, revision: Int)

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
    case .verify(let selection):
      return .verified(try await session.saveGitHub(selection))
    case .remove(let id, let revision):
      return .removed(try await session.removeGitHub(id: id, revision: revision))
    case .refreshCode(let id):
      return .snapshot(try await session.refreshGitHubCode(id: id))
    case .setRate(let requestsPerMinute, let revision):
      return .snapshot(
        try await session.updateGitHubRateLimit(
          requestsPerMinute: requestsPerMinute, revision: revision))
    }
  }
}

public enum GitHubResult: Equatable, Sendable {
  case snapshot(Components.Schemas.GitHubState)
  case credential(Components.Schemas.GitHubState)
  case page(Components.Schemas.GitHubDiscovery, replace: Bool)
  case saved(Components.Schemas.GitHubState)
  case verified(Components.Schemas.GitHubState)
  case removed(Components.Schemas.GitHubState)
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
  }

  public struct Draft: Equatable, Sendable {
    public var repository: String
    public var expectedId: Int64?
    public var branch: String
    public var intervalMinutes: Int
    public var features: Components.Schemas.GitHubFeatures

    init(defaults: Components.Schemas.GitHubDefaults) {
      repository = ""
      branch = ""
      intervalMinutes = defaults.intervalMinutes
      features = defaults.features
    }

    init(_ configured: Components.Schemas.GitHubConfiguredRepository) {
      repository = configured.repository.fullName
      expectedId = configured.repository.id
      branch = configured.branch
      intervalMinutes = configured.intervalMinutes
      features = configured.features
    }

    func selection(revision: Int) -> Components.Schemas.GitHubSelection {
      .init(
        revision: revision, repository: repository, expectedId: expectedId,
        branch: branch.isEmpty ? nil : branch, intervalMinutes: intervalMinutes, features: features)
    }
  }

  @ObservableState
  public struct State: Equatable {
    public internal(set) var managing = false
    public internal(set) var snapshot: Components.Schemas.GitHubState?
    public internal(set) var candidates: [Components.Schemas.GitHubRepository] = []
    public internal(set) var nextPage: Int?
    public internal(set) var draft: Draft?
    public internal(set) var panel: Panel?
    public internal(set) var busy = false
    public internal(set) var feedback: String?

    public init() {}
  }

  public enum Action {
    case manage
    case close
    case reload
    case refreshCode(Int64)
    case pollCollection
    case collectionPolled(Components.Schemas.GitHubState)
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
    case verifyAccess
    case chooseRepository(String)
    case backToPicker
    case dismissPanel
    case save
    case remove(Int64)
    case result(GitHubResult, feedback: String? = nil)
    case failed(String)
  }

  let perform: @Sendable (GitHubCommand) async throws -> GitHubResult
  private enum Cancellation { case collectionPoll }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .manage:
        state.managing = true
        return start(.load, state: &state)
      case .close:
        state.managing = false
        return .cancel(id: Cancellation.collectionPoll)
      case .reload:
        return start(.load, state: &state)
      case .refreshCode(let id):
        return start(.refreshCode(id), state: &state)
      case .pollCollection:
        guard state.managing, !state.busy else { return .none }
        return .run { [perform] send in
          if case .snapshot(let snapshot) = try? await perform(.load), !Task.isCancelled {
            await send(.collectionPolled(snapshot))
          }
        }
        .cancellable(id: Cancellation.collectionPoll, cancelInFlight: true)
      case .collectionPolled(let snapshot):
        guard state.managing, !state.busy else { return .none }
        updateSnapshot(snapshot, state: &state)
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
        guard let defaults = state.snapshot?.defaults, !state.busy else { return .none }
        state.feedback = nil
        state.panel = .picker
        state.draft = Draft(defaults: defaults)
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
        return start(.load, state: &state)
      case .setRate(let requestsPerMinute):
        guard let revision = state.snapshot?.revision else { return .none }
        return start(
          .setRate(requestsPerMinute: requestsPerMinute, revision: revision), state: &state)
      case .verifyAccess:
        guard let snapshot = state.snapshot, snapshot.credential == .connected,
          let configured = snapshot.repositories.first(where: {
            $0.repository.id == state.draft?.expectedId
          })
        else { return .none }
        let selection = Draft(configured).selection(revision: snapshot.revision)
        return start(.verify(selection), state: &state)
      case .edit(let repository):
        state.feedback = nil
        state.panel = .editOptions
        state.draft = Draft(repository)
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
        return .none
      case .dismissPanel:
        guard !state.busy else { return .none }
        state.feedback = nil
        if state.panel == .repositoryAccount {
          state.panel = .editOptions
        } else {
          state.panel = nil
          state.draft = nil
        }
        return .none
      case .save:
        guard let revision = state.snapshot?.revision, let draft = state.draft else { return .none }
        return start(.save(draft.selection(revision: revision)), state: &state)
      case .remove(let id):
        guard let revision = state.snapshot?.revision else { return .none }
        return start(.remove(id: id, revision: revision), state: &state)
      case .result(let result, let feedback):
        state.busy = false
        state.feedback = feedback
        switch result {
        case .snapshot(let snapshot):
          updateSnapshot(snapshot, state: &state)
        case .credential(let snapshot):
          state.snapshot = snapshot
          state.candidates = []
          state.nextPage = nil
          state.panel = state.panel == .repositoryAccount ? .editOptions : nil
        case .page(let page, let replace):
          if replace { state.candidates = [] }
          let previous = Set(state.candidates.map(\.id))
          state.candidates.append(
            contentsOf: page.repositories.filter { !previous.contains($0.id) })
          state.nextPage = page.nextPage
          if let traffic = page.traffic { state.snapshot?.traffic = traffic }
        case .saved(let snapshot):
          state.snapshot = snapshot
          state.draft = nil
          state.panel = nil
        case .verified(let snapshot):
          state.snapshot = snapshot
        case .removed(let snapshot):
          state.snapshot = snapshot
          state.draft = nil
          state.panel = nil
        }
        return .none
      case .failed(let message):
        state.busy = false
        state.feedback = message
        return .none
      }
    }
  }

  private func updateSnapshot(_ snapshot: Components.Schemas.GitHubState, state: inout State) {
    guard snapshot.revision >= (state.snapshot?.revision ?? 0),
      state.draft == nil || snapshot.revision == state.snapshot?.revision
    else { return }
    state.snapshot = snapshot
    if snapshot.credential != .connected {
      state.candidates = []
      state.nextPage = nil
    }
  }

  private func start(_ command: GitHubCommand, state: inout State) -> EffectOf<Self> {
    guard !state.busy else { return .none }
    state.busy = true
    state.feedback = nil
    return .merge(
      .cancel(id: Cancellation.collectionPoll),
      .run { [perform] send in
        do {
          await send(.result(try await perform(command)))
        } catch let rejection as GitHubRejection {
          if [
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
          await send(.failed(error.localizedDescription))
        }
      })
  }
}
