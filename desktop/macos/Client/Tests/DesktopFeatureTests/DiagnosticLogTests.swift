import ComposableArchitecture
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func diagnosticLogFiltersAndCursorAreSentTogether() async {
  let clock = TestClock()
  let first = Components.Schemas.SignalSample(id: 5, timestamp: 0, name: "api_query", level: .debug)
  let older = Components.Schemas.SignalSample(id: 3, timestamp: 0, name: "api_query", level: .debug)
  let error = BackendConnectionError.helperExited
  let store = TestStore(initialState: DiagnosticLogs.State()) {
    DiagnosticLogs { query, level, before in
      if query == "broken" { throw error }
      #expect(level == .debug)
      if query.isEmpty { return .init(records: []) }
      #expect(query == "query")
      if let before {
        #expect(before == first.id)
        return .init(records: [older])
      }
      return .init(records: [first], nextBeforeId: first.id)
    }
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.levelChanged(.debug)) {
    $0.level = .debug
    $0.isLoading = true
  }
  await store.receive(\.pageLoaded) { $0.isLoading = false }
  await store.send(.queryChanged("query")) {
    $0.query = "query"
    $0.isLoading = true
  }
  await clock.advance(by: .milliseconds(300))
  await store.receive(\.pageLoaded) {
    $0.isLoading = false
    $0.records = [first]
    $0.nextBeforeId = first.id
  }
  await store.send(.loadMore) { $0.isLoading = true }
  await store.receive(\.pageLoaded) {
    $0.isLoading = false
    $0.records = [first, older]
    $0.nextBeforeId = nil
  }
  await store.send(.queryChanged("broken")) {
    $0.query = "broken"
    $0.records = []
    $0.isLoading = true
  }
  await clock.advance(by: .milliseconds(300))
  await store.receive(\.failed) {
    $0.isLoading = false
    $0.feedback = error.localizedDescription
  }
}

@MainActor
@Test
func leavingLogsCancelsThePendingSearchAndReturningReloadsIt() async {
  let clock = TestClock()
  let record = Components.Schemas.SignalSample(
    id: 7, timestamp: 0, name: "api_query", level: .debug)
  var initial = Dashboard.State()
  initial.connection = .ready
  initial.diagnosticTab = .logs
  let store = TestStore(initialState: initial) {
    Dashboard(
      client: .init(
        snapshot: { throw BackendConnectionError.helperMissing },
        save: { $0 }, restart: { throw BackendConnectionError.helperMissing },
        exits: { AsyncStream { $0.finish() } },
        diagnosticLogs: { query, level, before in
          #expect(level == nil)
          #expect(before == nil)
          return .init(records: query == "api_query" ? [record] : [])
        }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.destinationChanged(.diagnostics)) { $0.destination = .diagnostics }
  await store.receive(\.diagnosticLogs.refresh) { $0.diagnosticLogs.isLoading = true }
  await store.receive(\.diagnosticLogs.pageLoaded) { $0.diagnosticLogs.isLoading = false }
  await store.send(.diagnosticLogs(.queryChanged("api_query"))) {
    $0.diagnosticLogs.query = "api_query"
    $0.diagnosticLogs.isLoading = true
  }
  await store.send(.diagnosticTabChanged(.metrics)) { $0.diagnosticTab = .metrics }
  await store.receive(\.diagnosticLogs.cancel) { $0.diagnosticLogs.isLoading = false }
  await clock.advance(by: .milliseconds(300))
  await store.finish()
  await store.send(.diagnosticTabChanged(.logs)) { $0.diagnosticTab = .logs }
  await store.receive(\.diagnosticLogs.refresh) { $0.diagnosticLogs.isLoading = true }
  await store.receive(\.diagnosticLogs.pageLoaded) {
    $0.diagnosticLogs.records = [record]
    $0.diagnosticLogs.isLoading = false
  }
}

@MainActor
@Test(.timeLimit(.minutes(1)))
func aLatePageCannotReplaceTheCurrentFilterResults() async throws {
  let (requests, started) =
    AsyncStream<CheckedContinuation<Components.Schemas.DiagnosticLogPage, Never>>.makeStream()
  var iterator = requests.makeAsyncIterator()
  let debugLog = Components.Schemas.SignalSample(
    id: 2, timestamp: 0, name: "api_query", level: .debug)
  let infoLog = Components.Schemas.SignalSample(
    id: 1, timestamp: 0, name: "backend_startup", level: .info)
  let store = TestStore(initialState: DiagnosticLogs.State()) {
    DiagnosticLogs { _, level, _ in
      if level == .debug { return .init(records: [debugLog]) }
      return await withCheckedContinuation { started.yield($0) }
    }
  }
  await store.send(.refresh) { $0.isLoading = true }
  let previous = try #require(await iterator.next())
  await store.send(.levelChanged(.debug)) { $0.level = .debug }
  await store.receive(\.pageLoaded) {
    $0.isLoading = false
    $0.records = [debugLog]
  }
  previous.resume(returning: .init(records: [infoLog]))
  started.finish()
  await store.finish()
  #expect(store.state.records == [debugLog])
}
