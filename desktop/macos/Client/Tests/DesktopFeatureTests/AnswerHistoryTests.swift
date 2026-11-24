import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func historySettingsAutomaticallySaveTheLatestEditUsingTheNewRevision() async {
  let initial = Components.Schemas.AnswerHistorySettings(
    revision: 1, enabled: false, retentionDays: 30)
  let saved = Components.Schemas.AnswerHistorySettings(
    revision: 2, enabled: true, retentionDays: 30)
  let latest = Components.Schemas.AnswerHistorySettings(
    revision: 3, enabled: true, retentionDays: 90)
  let (responses, response) = AsyncStream<Void>.makeStream()
  defer { response.finish() }
  let requests = LockIsolated<[Components.Schemas.AnswerHistorySettings]>([])
  let store = TestStore(initialState: AnswerHistory.State(settings: initial)) {
    AnswerHistory(
      client: .init(saveSettings: { input in
        requests.withValue { $0.append(input) }
        if input.revision == 1 {
          for await _ in responses {}
          return saved
        }
        return latest
      }))
  }
  await store.send(.enabledChanged(true)) {
    $0.draft?.enabled = true
    $0.submitted = $0.draft
  }
  await store.send(.retentionChanged(60)) { $0.draft?.retentionDays = 60 }
  await store.send(.retentionChanged(90)) { $0.draft?.retentionDays = 90 }
  response.finish()
  await store.receive(\.settingsSaved) {
    $0.confirmed = saved
    $0.draft?.revision = 2
    $0.submitted = $0.draft
  }
  await store.receive(\.settingsSaved) {
    $0.confirmed = latest
    $0.draft = latest
    $0.submitted = nil
  }
  #expect(
    requests.value == [
      .init(revision: 1, enabled: true, retentionDays: 30),
      .init(revision: 2, enabled: true, retentionDays: 90),
    ])
}

@MainActor
@Test
func historySettingsRestoreTheConfirmedPreferenceAfterAFailedSave() async {
  let initial = Components.Schemas.AnswerHistorySettings(
    revision: 2, enabled: true, retentionDays: 30)
  let failure = URLError(.cannotConnectToHost)
  let store = TestStore(initialState: AnswerHistory.State(settings: initial)) {
    AnswerHistory(client: .init(saveSettings: { _ in throw failure }))
  }
  await store.send(.retentionChanged(0))
  await store.send(.retentionChanged(7)) {
    $0.draft?.retentionDays = 7
    $0.submitted = $0.draft
  }
  await store.receive(\.saveFailed) {
    $0.draft = initial
    $0.submitted = nil
    $0.settingsFeedback = failure.localizedDescription
  }
}

@MainActor
@Test
func clearingInvalidatesAnOlderHistoryPageResponse() async {
  let id = UUID()
  let page = Components.Schemas.AnswerHistoryPage(entries: [
    .init(id: 1, question: "Private question", completedAtMs: 1, outcome: .answered)
  ])
  var initial = AnswerHistory.State()
  initial.listRequest = id
  initial.loading = true
  let (responses, response) = AsyncStream<Void>.makeStream()
  let store = TestStore(initialState: initial) {
    AnswerHistory(client: .init(clear: { for await _ in responses {} }))
  }
  await store.send(.clear) {
    $0.clearing = true
    $0.listRequest = nil
    $0.loading = false
  }
  await store.send(.pageLoaded(id, false, page))
  response.finish()
  await store.receive(\.cleared) {
    $0.clearing = false
    $0.settingsFeedback = "Answer history cleared."
  }
}

@MainActor
@Test
func historyKeepsPageOrderAndRefusesToDisplayAnExpiredSelection() async {
  let id = UUID()
  let newest = Components.Schemas.AnswerHistorySummary(
    id: 26, question: "The latest question", completedAtMs: 26, outcome: .answered)
  let older = Components.Schemas.AnswerHistorySummary(
    id: 25, question: "The earlier question", completedAtMs: 25, outcome: .insufficientEvidence)
  let failure = URLError(.resourceUnavailable)
  let store = TestStore(initialState: AnswerHistory.State()) {
    AnswerHistory(
      client: .init(
        list: { try historyPage(before: $0, newest: newest, older: older) },
        detail: { selected in
          #expect(selected == older.id)
          throw failure
        }))
  } withDependencies: {
    $0.uuid = .constant(id)
  }
  await store.send(.reload) {
    $0.listRequest = id
    $0.loading = true
  }
  await store.receive(\.pageLoaded) {
    $0.listRequest = nil
    $0.loading = false
    $0.entries = [newest]
    $0.nextBeforeId = newest.id
  }
  await store.send(.loadMore) {
    $0.listRequest = id
    $0.loading = true
  }
  await store.receive(\.pageLoaded) {
    $0.listRequest = nil
    $0.loading = false
    $0.entries = [newest, older]
    $0.nextBeforeId = nil
  }
  await store.send(.loadMore)
  await store.send(.select(older.id)) {
    $0.detailRequest = id
    $0.loadingDetail = true
  }
  await store.receive(\.detailFailed) {
    $0.detailRequest = nil
    $0.loadingDetail = false
    $0.feedback = failure.localizedDescription
  }
}

private func historyPage(
  before cursor: Int64?,
  newest: Components.Schemas.AnswerHistorySummary,
  older: Components.Schemas.AnswerHistorySummary
) throws -> Components.Schemas.AnswerHistoryPage {
  guard let cursor else { return .init(entries: [newest], nextBeforeId: newest.id) }
  try #require(cursor == newest.id)
  return .init(entries: [older])
}

@MainActor
@Test
func shorteningHistoryRetentionDiscardsCachedContentAndLateDetails() async {
  let oldSettings = Components.Schemas.AnswerHistorySettings(
    revision: 3, enabled: true, retentionDays: 30)
  let newSettings = Components.Schemas.AnswerHistorySettings(
    revision: 4, enabled: true, retentionDays: 1)
  let answer = Components.Schemas.LocalAnswer(
    outcome: .answered, model: "fixture", mode: .bm25, text: "Saved answer", citations: [])
  let entry = Components.Schemas.AnswerHistoryEntry(
    id: 1, question: "Saved question", completedAtMs: 1, scope: .init(), answer: answer)
  let id = UUID()
  var state = AnswerHistory.State(settings: oldSettings)
  state.entries = [.init(id: 1, question: entry.question, completedAtMs: 1, outcome: .answered)]
  state.selected = entry
  state.detailRequest = id
  state.loadingDetail = true
  let store = TestStore(initialState: state) {
    AnswerHistory(client: .init(loadSettings: { newSettings }))
  }
  await store.send(.loadSettings)
  await store.receive(\.settingsLoaded) {
    $0.confirmed = newSettings
    $0.draft = newSettings
    $0.entries = []
    $0.selected = nil
    $0.detailRequest = nil
    $0.loadingDetail = false
  }
  await store.send(.detailLoaded(id, entry))
  await store.send(.settingsLoaded(oldSettings))
}
