import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test(arguments: [true, false])
func confirmedGitHubPurgeInvalidatesOnlyDependentAnswers(usesPurgedRepository: Bool) async {
  var answer = Components.Schemas.LocalAnswer(
    outcome: .answered, model: "q4", mode: .bm25, text: "Fixture answer", citations: [])
  answer.evidenceRepositoryIds = usesPurgedRepository ? [41, 42] : [42]
  let budget = Components.Schemas.GitHubStorageBudget(revision: 1, limitGb: 10, bytes: 120)
  var state = Dashboard.State()
  state.answers.answer = answer
  state.answers.answerTraceID = "fixture-trace"
  state.answers.question = "Keep this draft"
  state.answerHistory.entries = [
    .init(id: 1, question: "Fixture question", completedAtMs: 1, outcome: .answered)
  ]
  state.answerHistory.selected = .init(
    id: 1, question: "Fixture question", completedAtMs: 1, scope: .init(repositoryId: 41),
    answer: answer)
  let store = TestStore(initialState: state) {
    Dashboard(
      client: .init(
        snapshot: { throw BackendConnectionError.helperExited }, save: { $0 },
        restart: { throw BackendConnectionError.helperExited },
        exits: { AsyncStream { $0.finish() } }))
  }
  await store.send(.github(.result(.purged(41, budget)))) {
    $0.github.storage = budget
    $0.answerHistory.entries = []
    $0.answerHistory.selected = nil
  }
  if usesPurgedRepository {
    await store.receive(\.answers.repositoryDataPurged) {
      $0.answers.answer = nil
      $0.answers.answerTraceID = nil
      $0.answers.answerFeedback = "Local repository data used by this answer was deleted."
    }
  } else {
    await store.receive(\.answers.repositoryDataPurged)
  }
  #expect(store.state.answers.question == "Keep this draft")
  #expect(store.state.answers.answer == (usesPurgedRepository ? nil : answer))
  #expect(store.state.answers.answerTraceID == (usesPurgedRepository ? nil : "fixture-trace"))
}

@MainActor
@Test
func aSecondInstanceGetsAnActionableStartupState() async {
  let store = TestStore(initialState: Dashboard.State()) {
    Dashboard(
      client: DashboardClient(
        snapshot: { throw BackendConnectionError.dataDirectoryInUse },
        save: { $0 }, restart: { throw BackendConnectionError.dataDirectoryInUse },
        exits: { AsyncStream { $0.finish() } }))
  }
  await store.send(.refresh)
  await store.receive(\.backendExited) {
    $0.connection = .dataDirectoryInUse
  }
  #expect(
    store.state.connection.failureMessage
      == BackendConnectionError.dataDirectoryInUse.localizedDescription)
}

@MainActor
@Test
func refreshAdoptsBackendSnapshot() async {
  let snapshot = DashboardSnapshot(
    status: .init(
      status: .ready, apiVersion: ._1, connectedSources: 0,
      indexedDocuments: 7, outboundAttempts: 11, inference: .notConfigured
    ),
    settings: .init(revision: 2, answerLanguage: .auto, diagnosticRetentionHours: 24),
    diagnostics: []
  )
  let store = TestStore(initialState: Dashboard.State()) {
    Dashboard(
      client: DashboardClient(
        snapshot: { snapshot }, save: { $0 }, restart: { snapshot },
        exits: { AsyncStream { $0.finish() } }
      ))
  }
  await store.send(.refresh)
  await store.receive(\.loaded) {
    $0.snapshot = snapshot
    $0.settingsForm = .init(confirmed: snapshot.settings)
    $0.connection = .ready
  }
}

@MainActor
@Test(arguments: [BackendConnectionError.helperMissing, .incompatibleAPI])
func reportsLauncherFailureThroughTheStore(reason: BackendConnectionError) async {
  let store = TestStore(initialState: Dashboard.State()) {
    Dashboard(
      client: DashboardClient(
        snapshot: { throw reason },
        save: { $0 },
        restart: { throw BackendConnectionError.helperMissing },
        exits: { AsyncStream { $0.finish() } }
      )
    )
  }
  await store.send(.refresh)
  await store.receive(\.loadFailed) {
    $0.connection = .unavailable(reason.localizedDescription)
  }
}

@MainActor
@Test
func overviewUpdatesTheEmbeddingIndexWithoutReconnectingAndStopsOnNavigation() async {
  let snapshot = DashboardSnapshot(
    status: .init(
      status: .ready, apiVersion: ._1, connectedSources: 1, indexedDocuments: 370,
      outboundAttempts: 663, inference: .notConfigured,
      retrieval: .init(state: .notConfigured, dimensions: 0, indexed: 0, total: 0)),
    settings: .init(revision: 3, answerLanguage: .fr, diagnosticRetentionHours: 24),
    diagnostics: [])
  var state = Dashboard.State(settings: snapshot.settings)
  state.connection = .ready
  state.snapshot = snapshot
  var indexing = snapshot.status
  indexing.retrieval = .init(state: .incomplete, dimensions: 1024, indexed: 400, total: 893)
  var ready = indexing
  ready.retrieval = .init(state: .ready, dimensions: 1024, indexed: 893, total: 893)
  let updates = [indexing, ready]
  let reads = LockIsolated(0)
  let clock = TestClock()
  let store = TestStore(initialState: state) {
    Dashboard(
      client: .init(
        snapshot: {
          Issue.record("Overview fetched the full dashboard snapshot.")
          return snapshot
        },
        save: { $0 }, restart: { snapshot }, exits: { AsyncStream { $0.finish() } },
        readStatus: {
          let count = reads.withValue {
            $0 += 1
            return $0
          }
          return updates[min(count, updates.count) - 1]
        }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.observeOverview)
  await store.receive(\.overviewStatusLoaded) { $0.snapshot?.status = updates[0] }
  await clock.advance(by: .seconds(3))
  await store.receive(\.overviewStatusLoaded) { $0.snapshot?.status = updates[1] }
  await store.send(.destinationChanged(.settings)) { $0.destination = .settings }
  await clock.advance(by: .seconds(9))
  await store.finish()
  #expect(reads.value == 2)
}

@MainActor
@Test
func reportsUnexpectedExitAndRecoversThroughExplicitRestart() async {
  let snapshot = DashboardSnapshot(
    status: .init(
      status: .ready, apiVersion: ._1, connectedSources: 0,
      indexedDocuments: 0, outboundAttempts: 0, inference: .notConfigured
    ),
    settings: .init(revision: 2, answerLanguage: .fr, diagnosticRetentionHours: 48),
    diagnostics: []
  )
  let (events, producer) = AsyncStream.makeStream(of: BackendConnectionError.self)
  let store = TestStore(initialState: Dashboard.State()) {
    Dashboard(
      client: DashboardClient(
        snapshot: { throw BackendConnectionError.incompatibleAPI }, save: { $0 },
        restart: { snapshot }, exits: { events }
      ))
  }
  await store.send(.observeBackend)
  producer.yield(.helperExited)
  await store.receive(\.backendExited) {
    $0.connection = .unavailable(
      "The local backend exited. Check that its data directory is available.")
  }
  await store.send(.restart) {
    $0.connection = .connecting
  }
  await store.receive(\.loaded) {
    $0.snapshot = snapshot
    $0.settingsForm = .init(confirmed: snapshot.settings)
    $0.connection = .ready
  }
  producer.finish()
  await store.finish()
}

@MainActor
@Test
func telemetryControlsAdoptBackendStateAndReportFailure() async {
  let snapshot = DashboardSnapshot(
    status: .init(
      status: .ready, apiVersion: ._1, connectedSources: 0,
      indexedDocuments: 0, outboundAttempts: 0, inference: .notConfigured
    ),
    settings: .init(revision: 0, answerLanguage: .auto, diagnosticRetentionHours: 24),
    diagnostics: []
  )
  let telemetry = Components.Schemas.TelemetrySnapshot(
    logs: [], metrics: [], traces: [],
    health: .init(
      state: .gaps, dropped: 2, pruned: 4, queued: 0, fileBytes: 100,
      budgetBytes: 200, retentionHours: 24, debugUntil: 1234)
  )
  let store = TestStore(initialState: Dashboard.State()) {
    Dashboard(
      client: DashboardClient(
        snapshot: { snapshot }, save: { $0 }, restart: { snapshot },
        exits: { AsyncStream { $0.finish() } },
        telemetry: { command in
          guard command == .enableDebug else { throw BackendConnectionError.helperMissing }
          return telemetry
        }
      ))
  }
  await store.send(.refresh)
  await store.receive(\.loaded) {
    $0.snapshot = snapshot
    $0.settingsForm = .init(confirmed: snapshot.settings)
    $0.connection = .ready
  }
  await store.send(.diagnosticTabChanged(.metrics)) {
    $0.diagnosticTab = .metrics
  }
  await store.send(.telemetryCommand(.enableDebug)) {
    $0.telemetryBusy = true
  }
  await store.receive(\.telemetryUpdated) {
    $0.telemetryBusy = false
    $0.snapshot?.telemetry = telemetry
  }
  await store.send(.telemetryCommand(.purgeExpired)) {
    $0.telemetryBusy = true
  }
  await store.receive(\.telemetryFailed) {
    $0.telemetryBusy = false
    $0.telemetryFeedback = "The bundled backend is missing."
  }
}

@MainActor
@Test
func changingDestinationKeepsTheQuestionDraft() async {
  let store = TestStore(initialState: Dashboard.State()) {
    Dashboard(
      client: DashboardClient(
        snapshot: { throw BackendConnectionError.helperMissing }, save: { $0 },
        restart: { throw BackendConnectionError.helperMissing },
        exits: { AsyncStream { $0.finish() } }
      ))
  }
  await store.send(.destinationChanged(.answers)) { $0.destination = .answers }
  await store.send(.answers(.questionChanged("How is traffic paced?"))) {
    $0.answers.question = "How is traffic paced?"
  }
  await store.send(.destinationChanged(.diagnostics)) { $0.destination = .diagnostics }
  await store.send(.destinationChanged(.answers)) { $0.destination = .answers }
}

@MainActor
@Test
func confirmingHistoryClearInvalidatesInspectorReadStartedDuringClear() async {
  let request = UUID()
  let answer = Components.Schemas.LocalAnswer(
    outcome: .answered, model: "fixture", mode: .bm25, text: "Private answer", citations: [])
  let entry = Components.Schemas.AnswerHistoryEntry(
    id: 1, question: "Private question", completedAtMs: 1, scope: .init(), answer: answer)
  var state = Dashboard.State()
  state.answerHistory.clearing = true
  state.traceInspector.traceID = "trace"
  state.traceInspector.savedAnswerRequest = request
  state.traceInspector.loadingSavedAnswer = true
  let store = TestStore(initialState: state) {
    Dashboard(
      client: .init(
        snapshot: { throw BackendConnectionError.helperExited }, save: { $0 },
        restart: { throw BackendConnectionError.helperExited },
        exits: { AsyncStream { $0.finish() } }))
  }
  await store.send(.answerHistory(.cleared)) {
    $0.answerHistory.clearing = false
    $0.answerHistory.settingsFeedback = "Answer history cleared."
  }
  await store.receive(\.traceInspector.discardSavedAnswer) {
    $0.traceInspector.savedAnswerRequest = nil
    $0.traceInspector.loadingSavedAnswer = false
  }
  await store.send(.traceInspector(.savedAnswerLoaded(request, entry)))
}
