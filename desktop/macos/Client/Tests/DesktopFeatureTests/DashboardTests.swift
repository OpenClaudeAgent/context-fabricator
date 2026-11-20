import ComposableArchitecture
import LocalAPI
import Testing

@testable import DesktopFeature

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
