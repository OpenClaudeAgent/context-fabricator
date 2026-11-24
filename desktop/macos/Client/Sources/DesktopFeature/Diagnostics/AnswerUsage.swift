import ComposableArchitecture
import LocalAPI

@Reducer
public struct AnswerUsage {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var snapshot: Components.Schemas.AnswerUsage?
    public internal(set) var isLoading = false
    public internal(set) var feedback: String?
  }

  public enum Action {
    case refresh
    case reset
    case loaded(Components.Schemas.AnswerUsage)
    case failed(String)
  }

  public typealias Read = @Sendable () async throws -> Components.Schemas.AnswerUsage
  private enum Cancellation { case request }
  private let read: Read

  public init(read: @escaping Read) { self.read = read }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .refresh:
        state.isLoading = true
        state.feedback = nil
        return .run { [read] send in
          do {
            let snapshot = try await read()
            try Task.checkCancellation()
            await send(.loaded(snapshot))
          } catch {
            if !Task.isCancelled { await send(.failed(error.localizedDescription)) }
          }
        }.cancellable(id: Cancellation.request, cancelInFlight: true)
      case .reset:
        state = State()
        return .cancel(id: Cancellation.request)
      case .loaded(let snapshot):
        guard state.isLoading else { return .none }
        state.isLoading = false
        state.snapshot = snapshot
        return .none
      case .failed(let message):
        guard state.isLoading else { return .none }
        state.isLoading = false
        state.feedback = message
        return .none
      }
    }
  }
}
