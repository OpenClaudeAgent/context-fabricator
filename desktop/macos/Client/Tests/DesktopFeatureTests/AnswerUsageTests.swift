import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func answerUsageLoadsOnlyWhenVisibleAndResetsOnLeaving() async throws {
  let fixture = """
    {"firstAt":100,"lastAt":200,"groups":[{"client":"desktop","modelProfile":"model-a",
    "latestTraceId":"0123456789abcdef0123456789abcdef","published":3,"abstained":1,"cancelled":2,"failed":0,
    "unknown":0,"p50Micros":500000,"p95Micros":900000}]}
    """
  let snapshot = try JSONDecoder().decode(
    Components.Schemas.AnswerUsage.self, from: Data(fixture.utf8))
  var initial = Dashboard.State()
  initial.connection = .ready
  let store = TestStore(initialState: initial) {
    Dashboard(
      client: .init(
        snapshot: { throw BackendConnectionError.helperMissing }, save: { $0 },
        restart: { throw BackendConnectionError.helperMissing },
        exits: { AsyncStream { $0.finish() } }, answerUsage: { snapshot }))
  }
  await store.send(.diagnosticTabChanged(.answerUsage)) { $0.diagnosticTab = .answerUsage }
  await store.send(.destinationChanged(.diagnostics)) { $0.destination = .diagnostics }
  await store.receive(\.answerUsage.refresh) { $0.answerUsage.isLoading = true }
  await store.receive(\.answerUsage.loaded) {
    $0.answerUsage.isLoading = false
    $0.answerUsage.snapshot = snapshot
  }
  #expect(snapshot.groups.first?.modelProfile == "model-a")
  #expect(snapshot.groups.first?.p95Micros == 900000)
  await store.send(.destinationChanged(.overview)) { $0.destination = .overview }
  await store.receive(\.answerUsage.reset) { $0.answerUsage.snapshot = nil }
}
