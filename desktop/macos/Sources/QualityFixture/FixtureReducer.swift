import ComposableArchitecture

@Reducer
struct FixtureReducer {
  @ObservableState
  struct State: Equatable {
    var isLoading = false
    var response: String?
  }

  enum Action: Equatable {
    case load
    case response(String)
  }

  let fetch: @Sendable () async -> String

  var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .load:
        state.isLoading = true
        return .run { [fetch] send in
          await send(.response(fetch()))
        }
      case .response(let value):
        state.isLoading = false
        state.response = value
        return .none
      }
    }
  }
}
