import ComposableArchitecture
import Foundation
import LocalAPI

public struct AnswerHistoryClient: Sendable {
  public var loadSettings: @Sendable () async throws -> Components.Schemas.AnswerHistorySettings
  public var saveSettings:
    @Sendable (Components.Schemas.AnswerHistorySettings) async throws ->
      Components.Schemas.AnswerHistorySettings
  public var list: @Sendable (Int64?) async throws -> Components.Schemas.AnswerHistoryPage
  public var detail: @Sendable (Int64) async throws -> Components.Schemas.AnswerHistoryEntry
  public var byTrace: @Sendable (String) async throws -> Components.Schemas.AnswerHistoryEntry?
  public var clear: @Sendable () async throws -> Void

  @preconcurrency public init(
    loadSettings: @escaping @Sendable () async throws -> Components.Schemas.AnswerHistorySettings =
      { throw BackendConnectionError.helperExited },
    saveSettings:
      @escaping @Sendable (Components.Schemas.AnswerHistorySettings) async throws ->
      Components.Schemas.AnswerHistorySettings = { _ in throw BackendConnectionError.helperExited },
    list: @escaping @Sendable (Int64?) async throws -> Components.Schemas.AnswerHistoryPage = { _ in
      throw BackendConnectionError.helperExited
    },
    detail: @escaping @Sendable (Int64) async throws -> Components.Schemas.AnswerHistoryEntry = {
      _ in throw BackendConnectionError.helperExited
    },
    byTrace: @escaping @Sendable (String) async throws -> Components.Schemas.AnswerHistoryEntry? = {
      _ in nil
    },
    clear: @escaping @Sendable () async throws -> Void = {
      throw BackendConnectionError.helperExited
    }
  ) {
    self.loadSettings = loadSettings
    self.saveSettings = saveSettings
    self.list = list
    self.detail = detail
    self.byTrace = byTrace
    self.clear = clear
  }
}

@Reducer
public struct AnswerHistory {
  @ObservableState
  public struct State: Equatable {
    public internal(set) var draft: Components.Schemas.AnswerHistorySettings?
    public internal(set) var confirmed: Components.Schemas.AnswerHistorySettings?
    var submitted: Components.Schemas.AnswerHistorySettings?
    public internal(set) var settingsFeedback: String?
    public internal(set) var entries: [Components.Schemas.AnswerHistorySummary] = []
    public internal(set) var nextBeforeId: Int64?
    public internal(set) var selected: Components.Schemas.AnswerHistoryEntry?
    public internal(set) var feedback: String?
    public internal(set) var loading = false
    public internal(set) var loadingDetail = false
    public internal(set) var clearing = false
    var listRequest: UUID?
    var detailRequest: UUID?
    public var saving: Bool { submitted != nil }
    public init(settings: Components.Schemas.AnswerHistorySettings? = nil) {
      self.draft = settings
      self.confirmed = settings
    }

    mutating func discardCachedEntries() {
      entries = []
      nextBeforeId = nil
      selected = nil
      listRequest = nil
      detailRequest = nil
      loading = false
      loadingDetail = false
    }
  }
  public enum Action {
    case loadSettings
    case settingsLoaded(Components.Schemas.AnswerHistorySettings)
    case settingsFailed(String)
    case enabledChanged(Bool)
    case retentionChanged(Int)
    case settingsSaved(Components.Schemas.AnswerHistorySettings)
    case saveFailed(String)
    case discardCachedEntries
    case reload, loadMore
    case pageLoaded(UUID, Bool, Components.Schemas.AnswerHistoryPage)
    case pageFailed(UUID, String)
    case select(Int64)
    case detailLoaded(UUID, Components.Schemas.AnswerHistoryEntry)
    case detailFailed(UUID, String)
    case clear, cleared
    case clearFailed(String)
  }
  private let client: AnswerHistoryClient
  @Dependency(\.uuid) var uuid
  public init(client: AnswerHistoryClient) { self.client = client }
  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .loadSettings:
        return .run { [client] send in
          do { await send(.settingsLoaded(try await client.loadSettings())) } catch {
            await send(.settingsFailed(error.localizedDescription))
          }
        }
      case .settingsLoaded(let value):
        guard value.revision >= (state.confirmed?.revision ?? value.revision) else { return .none }
        let baseline = state.confirmed
        state.confirmed = value
        if let baseline, !value.enabled || value.retentionDays < baseline.retentionDays {
          state.discardCachedEntries()
        }
        if !state.saving {
          var draft = value
          if let old = state.draft, let baseline {
            if old.enabled != baseline.enabled { draft.enabled = old.enabled }
            if old.retentionDays != baseline.retentionDays {
              draft.retentionDays = old.retentionDays
            }
          }
          state.draft = draft
        }
        return .none
      case .settingsFailed(let message):
        state.settingsFeedback = message
        return .none
      case .enabledChanged(let enabled):
        guard !state.clearing else { return .none }
        state.draft?.enabled = enabled
        state.settingsFeedback = nil
        return persistSettings(&state)
      case .retentionChanged(let days):
        guard !state.clearing, (1...365).contains(days) else { return .none }
        state.draft?.retentionDays = days
        state.settingsFeedback = nil
        return persistSettings(&state)
      case .settingsSaved(let value):
        guard let submitted = state.submitted else { return .none }
        let baseline = state.confirmed ?? value
        let latest = baseline.revision > value.revision ? baseline : value
        var draft = latest
        if let current = state.draft {
          if current.enabled != submitted.enabled { draft.enabled = current.enabled }
          if current.retentionDays != submitted.retentionDays {
            draft.retentionDays = current.retentionDays
          }
        }
        state.confirmed = latest
        state.draft = draft
        state.submitted = nil
        if !latest.enabled || latest.retentionDays < baseline.retentionDays {
          state.discardCachedEntries()
        }
        return persistSettings(&state)
      case .saveFailed(let message):
        state.submitted = nil
        state.draft = state.confirmed
        state.settingsFeedback = message
        return .none
      case .discardCachedEntries:
        state.discardCachedEntries()
        return .none
      case .reload, .loadMore:
        guard !state.clearing else { return .none }
        let append: Bool = if case .loadMore = action { true } else { false }
        if append && (state.loading || state.nextBeforeId == nil) { return .none }
        if !append { state.discardCachedEntries() }
        let cursor = append ? state.nextBeforeId : nil
        let id = uuid()
        state.listRequest = id
        state.loading = true
        state.feedback = nil
        return .run { [client] send in
          do { await send(.pageLoaded(id, append, try await client.list(cursor))) } catch {
            await send(.pageFailed(id, error.localizedDescription))
          }
        }
      case .pageLoaded(let id, let append, let page):
        guard state.listRequest == id else { return .none }
        state.listRequest = nil
        state.loading = false
        let combined = append ? state.entries + page.entries : page.entries
        state.entries = Array(combined.suffix(250))
        state.nextBeforeId = page.nextBeforeId
        return .none
      case .pageFailed(let id, let message):
        guard state.listRequest == id else { return .none }
        state.listRequest = nil
        state.loading = false
        state.feedback = message
        return .none
      case .select(let entryID):
        guard !state.clearing else { return .none }
        let id = uuid()
        state.detailRequest = id
        state.loadingDetail = true
        state.selected = nil
        state.feedback = nil
        return .run { [client] send in
          do { await send(.detailLoaded(id, try await client.detail(entryID))) } catch {
            await send(.detailFailed(id, error.localizedDescription))
          }
        }
      case .detailLoaded(let id, let entry):
        guard state.detailRequest == id else { return .none }
        state.detailRequest = nil
        state.loadingDetail = false
        state.selected = entry
        return .none
      case .detailFailed(let id, let message):
        guard state.detailRequest == id else { return .none }
        state.detailRequest = nil
        state.loadingDetail = false
        state.feedback = message
        return .none
      case .clear:
        guard !state.clearing, !state.saving else { return .none }
        state.clearing = true
        state.settingsFeedback = nil
        state.feedback = nil
        state.listRequest = nil
        state.detailRequest = nil
        state.loading = false
        state.loadingDetail = false
        return .run { [client] send in
          do {
            try await client.clear()
            await send(.cleared)
          } catch { await send(.clearFailed(error.localizedDescription)) }
        }
      case .cleared:
        guard state.clearing else { return .none }
        state.clearing = false
        state.entries = []
        state.selected = nil
        state.nextBeforeId = nil
        state.settingsFeedback = "Answer history cleared."
        return .none
      case .clearFailed(let message):
        state.clearing = false
        state.settingsFeedback = message
        return .none
      }
    }
  }

  private func persistSettings(_ state: inout State) -> EffectOf<Self> {
    guard let draft = state.draft, let confirmed = state.confirmed, !state.saving,
      draft.enabled != confirmed.enabled || draft.retentionDays != confirmed.retentionDays
    else { return .none }
    state.submitted = draft
    return .run { [client] send in
      do { await send(.settingsSaved(try await client.saveSettings(draft))) } catch {
        await send(.saveFailed(error.localizedDescription))
      }
    }
  }
}
