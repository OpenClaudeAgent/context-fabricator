import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@Test
func diagnosticsDecodesServerOutcomesAndKeepsEarlierPayloadsReadable() throws {
  let payload = Data(
    """
    [
      {"id":1,"timestamp":1,"name":"api_answer","httpStatus":200,"level":"ok",
       "spanOutcome":"insufficientEvidence"},
      {"id":2,"timestamp":1,"name":"api_request","traceRequestKind":"backgroundCheck"},
      {"id":3,"timestamp":1,"name":"api_answer"}
    ]
    """.utf8)
  let samples = try JSONDecoder().decode([Components.Schemas.SignalSample].self, from: payload)
  #expect(samples.map(\.spanOutcome) == [.insufficientEvidence, nil, nil])
  #expect(samples.map(\.traceRequestKind) == [nil, .backgroundCheck, nil])
}

@Test
func answerPaneRequiresAnswerSpansFromTheDisplayedTrace() {
  var state = TraceInspector.State()
  state.traceID = "history"
  state.spans = [.init(id: 1, timestamp: 1, name: "github_history")]
  #expect(!state.isAnswerTrace("history"))
  state.traceID = "answer"
  state.spans = [.init(id: 2, timestamp: 2, name: "api_answer")]
  #expect(state.isAnswerTrace("answer"))
  #expect(!state.isAnswerTrace("history"))
}

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

@MainActor
@Test
func savedAnswerLoadsOnlyOnRequestAndExpires() async {
  let clock = TestClock()
  let reads = LockIsolated(0)
  let answer = Components.Schemas.LocalAnswer(
    outcome: .answered, model: "fixture", mode: .bm25, text: "Private answer", citations: [])
  let entry = Components.Schemas.AnswerHistoryEntry(
    id: 1, question: "Private question", completedAtMs: 1, expiresAtMs: 2000,
    scope: .init(), answer: answer)
  let span = Components.Schemas.SignalSample(id: 1, timestamp: 1, name: "root")
  let store = TestStore(initialState: TraceInspector.State()) {
    TraceInspector(readSavedAnswer: { _ in
      reads.withValue { $0 += 1 }
      return entry
    }) { _, _ in
      .init(spans: [span], logs: [])
    }
  } withDependencies: {
    $0.continuousClock = clock
    $0.date.now = Date(timeIntervalSince1970: 1)
    $0.uuid = .incrementing
  }
  await store.send(.load("trace")) {
    $0.traceID = "trace"
    $0.isLoading = true
  }
  await store.receive(\.loaded) {
    $0.spans = [span]
    $0.isLoading = false
  }
  #expect(reads.value == 0)
  await store.send(.loadSavedAnswer) {
    $0.savedAnswerRequest = UUID(0)
    $0.loadingSavedAnswer = true
  }
  await store.receive(\.savedAnswerLoaded) {
    $0.savedAnswerRequest = nil
    $0.loadingSavedAnswer = false
    $0.savedAnswer = entry
    $0.expiryToken = UUID(1)
  }
  await clock.advance(by: .seconds(1))
  await store.receive(\.savedAnswerExpired) {
    $0.savedAnswer = nil
    $0.expiryToken = nil
  }
  #expect(reads.value == 1)
}

@MainActor
@Test
func discardedSavedAnswerIgnoresLateResults() async {
  let request = UUID()
  let answer = Components.Schemas.LocalAnswer(
    outcome: .answered, model: "fixture", mode: .bm25, text: "Private answer", citations: [])
  let entry = Components.Schemas.AnswerHistoryEntry(
    id: 1, question: "Private question", completedAtMs: 1, scope: .init(), answer: answer)
  var state = TraceInspector.State()
  state.traceID = "trace"
  state.savedAnswerRequest = request
  state.loadingSavedAnswer = true
  let store = TestStore(initialState: state) {
    TraceInspector { _, _ in .init(spans: [], logs: []) }
  }
  await store.send(.discardSavedAnswer) {
    $0.savedAnswerRequest = nil
    $0.loadingSavedAnswer = false
  }
  await store.send(.savedAnswerLoaded(request, entry))
}

@MainActor
@Test(arguments: [false, true])
func missingOrFailedSavedAnswerDoesNotExposePrivateErrorDetails(fails: Bool) async {
  var state = TraceInspector.State()
  state.traceID = "trace"
  let store = TestStore(initialState: state) {
    TraceInspector(readSavedAnswer: { _ in
      if fails { throw BackendConnectionError.helperExited }
      return nil
    }) { _, _ in .init(spans: [], logs: []) }
  } withDependencies: {
    $0.uuid = .incrementing
  }
  await store.send(.loadSavedAnswer) {
    $0.savedAnswerRequest = UUID(0)
    $0.loadingSavedAnswer = true
  }
  if fails {
    await store.receive(\.savedAnswerFailed) {
      $0.savedAnswerRequest = nil
      $0.loadingSavedAnswer = false
      $0.savedAnswerFeedback = "Could not load the saved answer."
    }
  } else {
    await store.receive(\.savedAnswerLoaded) {
      $0.savedAnswerRequest = nil
      $0.loadingSavedAnswer = false
    }
  }
}
