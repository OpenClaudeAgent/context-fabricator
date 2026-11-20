import ComposableArchitecture
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func traceInspectorRetriesOnceThenLoadsOlderRecords() async {
  let clock = TestClock()
  let trace = "0123456789abcdef0123456789abcdef"
  let step = Components.Schemas.SignalSample(
    id: 3, timestamp: 1, name: "api_answer", value: 1370, level: .ok, traceId: trace)
  let log = Components.Schemas.SignalSample(
    id: 1, timestamp: 1, name: "answer_retrieval", level: .info, traceId: trace)
  let calls = LockIsolated<[Int64?]>([])
  let store = TestStore(initialState: TraceInspector.State()) {
    TraceInspector { id, before in
      #expect(id == trace)
      let count = calls.withValue { history in
        history.append(before)
        return history.count
      }
      if count == 1 { return .init(spans: [], logs: []) }
      if before == nil { return .init(spans: [step], logs: [], nextBeforeId: 3) }
      return .init(spans: [], logs: [log])
    }
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.load(trace)) {
    $0.traceID = trace
    $0.isLoading = true
  }
  await clock.advance(by: .milliseconds(350))
  await store.receive(\.loaded) {
    $0.spans = [step]
    $0.nextBeforeId = 3
    $0.isLoading = false
  }
  await store.send(.loadMore) { $0.isLoading = true }
  await store.receive(\.loaded) {
    $0.logs = [log]
    $0.nextBeforeId = nil
    $0.isLoading = false
  }
  #expect(calls.value == [nil, nil, 3])
  await store.send(.close) { $0 = .init() }
  await store.send(.loaded(trace, .init(spans: [step], logs: [])))
}

@MainActor
@Test
func traceInspectorSurfacesReadFailureAndRetriesAfterUserAction() async {
  let trace = "0123456789abcdef0123456789abcdef"
  let reads = LockIsolated(0)
  let step = Components.Schemas.SignalSample(
    id: 7, timestamp: 1, name: "api_answer", value: 0, traceId: trace)
  let store = TestStore(initialState: TraceInspector.State()) {
    TraceInspector { id, before in
      #expect(id == trace && before == nil)
      let first = reads.withValue { count in
        count += 1
        return count == 1
      }
      if first { throw BackendConnectionError.helperExited }
      return .init(spans: [step], logs: [])
    }
  }
  await store.send(.load(trace)) {
    $0.traceID = trace
    $0.isLoading = true
  }
  await store.receive(\.failed) {
    $0.isLoading = false
    $0.feedback = BackendConnectionError.helperExited.localizedDescription
  }
  await store.send(.load(trace)) {
    $0.feedback = nil
    $0.isLoading = true
  }
  await store.receive(\.loaded) {
    $0.spans = [step]
    $0.isLoading = false
  }
}

@MainActor
@Test
func unavailableTraceStopsAfterTheBoundedRetry() async {
  let clock = TestClock()
  let id = "0123456789abcdef0123456789abcdef"
  let reads = LockIsolated(0)
  let store = TestStore(initialState: TraceInspector.State()) {
    TraceInspector { _, _ in
      reads.withValue { $0 += 1 }
      return .init(spans: [], logs: [])
    }
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.load(id)) {
    $0.traceID = id
    $0.isLoading = true
  }
  await clock.advance(by: .milliseconds(350))
  await store.receive(\.loaded) { $0.isLoading = false }
  await clock.advance(by: .seconds(10))
  #expect(reads.value == 2)
  #expect(!store.state.hasRecords && store.state.feedback == nil)
  await store.finish()
}
