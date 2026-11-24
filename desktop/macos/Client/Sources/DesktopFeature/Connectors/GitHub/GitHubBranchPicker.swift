import ComposableArchitecture
import Foundation
import LocalAPI

@Reducer
public struct GitHubBranchPicker {
  public struct Target: Equatable, Sendable {
    public let id: Int64
    public let revision: Int
  }

  public enum Phase: Equatable {
    case idle
    case loading(Int)
    case ready
    case failed(Int, String)
  }

  @ObservableState
  public struct State: Equatable {
    public internal(set) var target: Target?
    public internal(set) var phase = Phase.idle
    public internal(set) var branches: [String] = []
    public internal(set) var nextPage: Int?

    public init() {}

    public func matchingBranches(_ query: String) -> [String] {
      let search = query.trimmingCharacters(in: .whitespacesAndNewlines)
      return search.isEmpty ? branches : branches.filter { $0.localizedStandardContains(search) }
    }
  }

  public enum Action {
    case open(Target)
    case close
    case loadMore
    case retry
    case select(String)
    case loaded(Target, page: Int, Components.Schemas.GitHubBranchPage)
    case failed(Target, page: Int, String)
  }

  let list: @Sendable (Target, Int) async throws -> Components.Schemas.GitHubBranchPage
  private enum Cancellation { case request }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .open(let target):
        guard state.target != target else { return .none }
        state = State()
        state.target = target
        return load(1, target: target, state: &state)
      case .close:
        state = State()
        return .cancel(id: Cancellation.request)
      case .loadMore:
        guard state.phase == .ready, let target = state.target, let page = state.nextPage
        else { return .none }
        return load(page, target: target, state: &state)
      case .retry:
        guard let target = state.target, case .failed(let page, _) = state.phase
        else { return .none }
        return load(page, target: target, state: &state)
      case .loaded(let target, let page, let response):
        guard state.target == target, state.phase == .loading(page) else { return .none }
        var seen = Set(state.branches)
        state.branches += response.branches.filter { seen.insert($0).inserted }
        state.nextPage = response.nextPage.flatMap { $0 > page ? $0 : nil }
        state.phase = .ready
        return .none
      case .failed(let target, let page, let message):
        guard state.target == target, state.phase == .loading(page) else { return .none }
        state.phase = .failed(page, message)
        return .none
      case .select:
        return .none
      }
    }
  }

  private func load(_ page: Int, target: Target, state: inout State) -> EffectOf<Self> {
    state.phase = .loading(page)
    return .run { [list] send in
      do {
        let response = try await list(target, page)
        try Task.checkCancellation()
        await send(.loaded(target, page: page, response))
      } catch is CancellationError {
        return
      } catch {
        guard !Task.isCancelled else { return }
        await send(.failed(target, page: page, error.localizedDescription))
      }
    }
    .cancellable(id: Cancellation.request, cancelInFlight: true)
  }
}
