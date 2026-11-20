import ComposableArchitecture
import Foundation
import LocalAPI

@Reducer
public struct TraceInspector {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var spans: [Components.Schemas.SignalSample] = []
    public internal(set) var logs: [Components.Schemas.SignalSample] = []
    public internal(set) var nextBeforeId: Int64?
    public internal(set) var isLoading = false
    public internal(set) var feedback: String?
    var traceID: String?

    public var hasRecords: Bool { !spans.isEmpty || !logs.isEmpty }
  }

  public enum Action {
    case load(String)
    case loadMore
    case close
    case loaded(String, Components.Schemas.DiagnosticTracePage)
    case failed(String, String)
  }

  public typealias Read =
    @Sendable (String, Int64?) async throws -> Components.Schemas.DiagnosticTracePage

  private enum Cancellation: Hashable { case page }
  @Dependency(\.continuousClock) private var clock
  private let read: Read

  public init(read: @escaping Read) { self.read = read }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .load(let id):
        state = State()
        state.traceID = id
        return load(&state, id: id)
      case .loadMore:
        guard let id = state.traceID, let before = state.nextBeforeId, !state.isLoading
        else { return .none }
        return load(&state, id: id, before: before)
      case .close:
        state = State()
        return .cancel(id: Cancellation.page)
      case .loaded(let id, let page):
        guard id == state.traceID else { return .none }
        state.spans.append(contentsOf: page.spans)
        state.logs.append(contentsOf: page.logs)
        state.nextBeforeId = page.nextBeforeId
        state.isLoading = false
        return .none
      case .failed(let id, let message):
        guard id == state.traceID else { return .none }
        state.isLoading = false
        state.feedback = message
        return .none
      }
    }
  }

  private func load(_ state: inout State, id: String, before: Int64? = nil) -> EffectOf<Self> {
    state.isLoading = true
    state.feedback = nil
    let retryClock = clock
    return .run { [read] send in
      do {
        var page = try await read(id, before)
        if before == nil && !page.spans.contains(where: { $0.parentSpanId == nil }) {
          // Telemetry writes run on the local sink's background flush interval.
          try await retryClock.sleep(for: .milliseconds(350))
          page = try await read(id, nil)
        }
        try Task.checkCancellation()
        await send(.loaded(id, page))
      } catch {
        if !Task.isCancelled { await send(.failed(id, error.localizedDescription)) }
      }
    }
    .cancellable(id: Cancellation.page, cancelInFlight: true)
  }
}
