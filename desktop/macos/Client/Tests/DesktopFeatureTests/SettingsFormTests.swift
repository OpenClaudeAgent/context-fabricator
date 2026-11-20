import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func aDelayedSavePreservesNewerEdits() async {
  let initial = Components.Schemas.Settings(
    revision: 1, answerLanguage: .en, diagnosticRetentionHours: 24)
  let (responses, response) = AsyncStream<Void>.makeStream()
  var submitted = initial
  submitted.diagnosticRetentionHours = 48
  let effective = Components.Schemas.Settings(
    revision: 2, answerLanguage: .en, diagnosticRetentionHours: 48)
  let store = TestStore(initialState: SettingsForm.State(confirmed: initial)) {
    SettingsForm { submitted in
      #expect(submitted.diagnosticRetentionHours == 48)
      for await _ in responses {}
      return effective
    }
  }
  await store.send(.retentionChanged(48)) { $0.draft = submitted }
  await store.send(.save) { $0.submitted = .init(draft: submitted, baseline: initial) }
  await store.send(.save)
  await store.send(.retentionChanged(72)) { $0.draft?.diagnosticRetentionHours = 72 }
  response.finish()
  await store.receive(\.saved) {
    $0.confirmed = effective
    $0.draft?.revision = 2
    $0.submitted = nil
    $0.feedback = "Settings saved."
  }
}

@MainActor
@Test
func aFailedSaveKeepsTheDraftAndPresentsTheBackendError() async {
  let initial = Components.Schemas.Settings(
    revision: 1, answerLanguage: .en, diagnosticRetentionHours: 24)
  let store = TestStore(initialState: SettingsForm.State(confirmed: initial)) {
    SettingsForm { _ in throw BackendConnectionError.helperExited }
  }
  await store.send(.answerLanguageChanged(.fr)) { $0.draft?.answerLanguage = .fr }
  await store.send(.retentionChanged(48)) { $0.draft?.diagnosticRetentionHours = 48 }
  await store.send(.storageLimitChanged(50)) { $0.draft?.diagnosticStorageLimitMb = 50 }
  var submitted = initial
  submitted.answerLanguage = .fr
  submitted.diagnosticRetentionHours = 48
  submitted.diagnosticStorageLimitMb = 50
  await store.send(.save) { $0.submitted = .init(draft: submitted, baseline: initial) }
  await store.receive(\.saveFailed) {
    $0.submitted = nil
    $0.feedback = BackendConnectionError.helperExited.localizedDescription
  }
}

@MainActor
@Test
func refreshMergesUntouchedFieldsAndRejectsOldRevisions() async {
  let initial = Components.Schemas.Settings(
    revision: 1, answerLanguage: .en, diagnosticRetentionHours: 24, diagnosticStorageLimitMb: 10)
  let newest = Components.Schemas.Settings(
    revision: 2, answerLanguage: .fr, diagnosticRetentionHours: 36, diagnosticStorageLimitMb: 80)
  let status = Components.Schemas.Status(
    status: .ready, apiVersion: ._1, connectedSources: 0,
    indexedDocuments: 0, outboundAttempts: 0, inference: .notConfigured)
  let fresh = DashboardSnapshot(status: status, settings: newest, diagnostics: [])
  let old = DashboardSnapshot(status: status, settings: initial, diagnostics: [])
  let store = TestStore(initialState: Dashboard.State(settings: initial)) {
    Dashboard(
      client: DashboardClient(
        snapshot: { fresh }, save: { $0 }, restart: { old },
        exits: { AsyncStream { $0.finish() } }
      ))
  }
  await store.send(\.settingsForm.retentionChanged, 72) {
    $0.settingsForm.draft?.diagnosticRetentionHours = 72
  }
  await store.send(\.settingsForm.storageLimitChanged, 100) {
    $0.settingsForm.draft?.diagnosticStorageLimitMb = 100
  }
  await store.send(.refresh)
  await store.receive(\.loaded) {
    $0.snapshot = fresh
    $0.connection = .ready
    $0.settingsForm.confirmed = newest
    $0.settingsForm.draft?.revision = 2
    $0.settingsForm.draft?.answerLanguage = .fr
  }
  await store.send(.restart) { $0.connection = .connecting }
  await store.receive(\.loaded) {
    $0.snapshot = old
    $0.connection = .ready
  }
}

@MainActor
@Test
func refreshDuringSavePreservesAnEditBackToThePreviousValue() async {
  let initial = Components.Schemas.Settings(
    revision: 1, answerLanguage: .en, diagnosticRetentionHours: 24)
  var submitted = initial
  submitted.diagnosticRetentionHours = 48
  let effective = Components.Schemas.Settings(
    revision: 2, answerLanguage: .en, diagnosticRetentionHours: 48)
  let snapshot = DashboardSnapshot(
    status: .init(
      status: .ready, apiVersion: ._1, connectedSources: 0,
      indexedDocuments: 0, outboundAttempts: 0, inference: .notConfigured),
    settings: effective, diagnostics: [])
  let (responses, response) = AsyncStream<Void>.makeStream()
  let store = TestStore(initialState: Dashboard.State(settings: initial)) {
    Dashboard(
      client: DashboardClient(
        snapshot: { snapshot },
        save: { _ in
          for await _ in responses {}
          return effective
        },
        restart: { snapshot },
        exits: { AsyncStream { $0.finish() } }
      ))
  }
  await store.send(\.settingsForm.retentionChanged, 48) { $0.settingsForm.draft = submitted }
  await store.send(\.settingsForm.save) {
    $0.settingsForm.submitted = .init(draft: submitted, baseline: initial)
  }
  await store.send(\.settingsForm.retentionChanged, 24) {
    $0.settingsForm.draft?.diagnosticRetentionHours = 24
  }
  await store.send(.refresh)
  await store.receive(\.loaded) {
    $0.snapshot = snapshot
    $0.connection = .ready
    $0.settingsForm.confirmed = effective
  }
  response.finish()
  await store.receive(\.settingsForm.saved) {
    $0.settingsForm.draft?.revision = 2
    $0.settingsForm.submitted = nil
    $0.settingsForm.feedback = "Settings saved."
  }
}
