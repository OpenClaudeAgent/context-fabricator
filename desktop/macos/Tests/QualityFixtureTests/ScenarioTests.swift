import ComposableArchitecture
import Testing

@testable import QualityFixture

@MainActor
@Test
func scenarioFixture() async {
  let store = TestStore(initialState: FixtureReducer.State()) {
    FixtureReducer(fetch: { "fixture response" })
  }
  await store.send(.load) {
    $0.isLoading = true
  }
  await store.receive(\.response) {
    $0.isLoading = false
    $0.response = "fixture response"
  }
  await store.finish()
}
