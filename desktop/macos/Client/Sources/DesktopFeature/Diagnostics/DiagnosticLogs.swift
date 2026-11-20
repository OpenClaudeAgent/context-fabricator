import ComposableArchitecture
import Foundation
import LocalAPI

@Reducer
public struct DiagnosticLogs {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var query = ""
    public internal(set) var level: Components.Schemas.DiagnosticLogLevel?
    public internal(set) var records: [Components.Schemas.SignalSample] = []
    public internal(set) var nextBeforeId: Int64?
    public internal(set) var isLoading = false
    public internal(set) var feedback: String?
  }

  public enum Action {
    case queryChanged(String)
    case levelChanged(Components.Schemas.DiagnosticLogLevel?)
    case refresh
    case loadMore
    case cancel
    case pageLoaded(Components.Schemas.DiagnosticLogPage, append: Bool)
    case failed(String)
  }

  public typealias Read =
    @Sendable (
      String, Components.Schemas.DiagnosticLogLevel?, Int64?
    ) async throws -> Components.Schemas.DiagnosticLogPage

  private enum Cancellation: Hashable { case page }
  private let read: Read
  @Dependency(\.continuousClock) private var clock

  public init(read: @escaping Read) { self.read = read }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .queryChanged(let query):
        guard state.query != query else { return .none }
        state.query = query
        return reset(&state, delay: .milliseconds(300))
      case .levelChanged(let level):
        guard state.level != level else { return .none }
        state.level = level
        return reset(&state)
      case .refresh:
        return load(&state)
      case .loadMore:
        guard let before = state.nextBeforeId, !state.isLoading else { return .none }
        return load(&state, before: before)
      case .cancel:
        state.isLoading = false
        return .cancel(id: Cancellation.page)
      case .pageLoaded(let page, let append):
        state.isLoading = false
        state.nextBeforeId = page.nextBeforeId
        if append {
          state.records.append(contentsOf: page.records)
        } else {
          state.records = page.records
        }
        return .none
      case .failed(let message):
        state.isLoading = false
        state.feedback = message
        return .none
      }
    }
  }

  private func reset(_ state: inout State, delay: Duration? = nil) -> EffectOf<Self> {
    state.records = []
    state.nextBeforeId = nil
    return load(&state, delay: delay)
  }

  private func load(_ state: inout State, before: Int64? = nil, delay: Duration? = nil)
    -> EffectOf<Self>
  {
    state.isLoading = true
    state.feedback = nil
    let searchClock = clock
    return .run { [read, query = state.query, level = state.level] send in
      do {
        if let delay { try await searchClock.sleep(for: delay) }
        let page = try await read(query, level, before)
        try Task.checkCancellation()
        await send(.pageLoaded(page, append: before != nil))
      } catch is CancellationError {
      } catch {
        if !Task.isCancelled { await send(.failed(error.localizedDescription)) }
      }
    }
    .cancellable(id: Cancellation.page, cancelInFlight: true)
  }
}
