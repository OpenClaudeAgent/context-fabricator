import ComposableArchitecture
import Foundation
import LocalAPI

@Reducer
public struct SettingsForm {
  @ObservableState
  public struct State: Equatable {
    struct PendingSave: Equatable {
      let draft: Components.Schemas.Settings
      let baseline: Components.Schemas.Settings
    }

    public internal(set) var embeddingModels: Components.Schemas.EmbeddingModelSelection?
    public internal(set) var embeddingLoading = false
    public internal(set) var embeddingSwitching = false
    public internal(set) var embeddingFeedback: String?
    public internal(set) var draft: Components.Schemas.Settings?
    public internal(set) var feedback: String?
    var confirmed: Components.Schemas.Settings?
    var submitted: PendingSave?

    public var saving: Bool { submitted != nil }

    init(confirmed: Components.Schemas.Settings? = nil) {
      self.confirmed = confirmed
      self.draft = confirmed
    }

    mutating func refresh(_ settings: Components.Schemas.Settings) {
      guard settings.revision >= (confirmed?.revision ?? settings.revision) else { return }
      if saving {
        // Keep the draft separate from refreshes while it is compared to the submitted values.
        confirmed = settings
      } else {
        rebase(on: settings, preservingEditsSince: confirmed.map { [$0] } ?? [])
      }
    }

    mutating func rebase(
      on settings: Components.Schemas.Settings,
      preservingEditsSince baselines: [Components.Schemas.Settings]
    ) {
      confirmed = settings
      var rebased = settings
      if let draft {
        func edited<Value: Equatable>(_ field: KeyPath<Components.Schemas.Settings, Value>) -> Bool
        {
          baselines.contains { $0[keyPath: field] != draft[keyPath: field] }
        }
        if edited(\.answerLanguage) {
          rebased.answerLanguage = draft.answerLanguage
        }
        if edited(\.answerLocale) {
          rebased.answerLocale = draft.answerLocale
        }
        if edited(\.diagnosticRetentionHours) {
          rebased.diagnosticRetentionHours = draft.diagnosticRetentionHours
        }
        if edited(\.inferenceArtifactLimitGb) {
          rebased.inferenceArtifactLimitGb = draft.inferenceArtifactLimitGb
        }
        if edited(\.diagnosticStorageLimitMb) {
          rebased.diagnosticStorageLimitMb = draft.diagnosticStorageLimitMb
        }
      }
      draft = rebased
    }
  }

  public enum Action {
    case loadEmbeddingModels
    case embeddingModelsLoaded(Components.Schemas.EmbeddingModelSelection)
    case embeddingModelsFailed(String)
    case selectEmbeddingModel(String)
    case embeddingModelSelected(Components.Schemas.EmbeddingModelSelection)
    case embeddingSelectionFailed(String)
    case answerLanguageChanged(Components.Schemas.Settings.answerLanguagePayload)
    case answerLocaleChanged(String)
    case retentionChanged(Int)
    case storageLimitChanged(Int)
    case inferenceArtifactLimitChanged(Int)
    case save
    case saved(Components.Schemas.Settings)
    case saveFailed(String)
  }

  var loadEmbeddingModels: @Sendable () async throws -> Components.Schemas.EmbeddingModelSelection =
    {
      throw BackendConnectionError.helperExited
    }
  var selectEmbeddingModel:
    @Sendable (String) async throws -> Components.Schemas.EmbeddingModelSelection =
      { _ in
        throw BackendConnectionError.helperExited
      }
  let save: @Sendable (Components.Schemas.Settings) async throws -> Components.Schemas.Settings

  private enum Cancellation { case embeddingLoad }

  public var body: some ReducerOf<Self> {
    Reduce { state, action in
      switch action {
      case .loadEmbeddingModels:
        guard !state.embeddingSwitching else { return .none }
        state.embeddingLoading = true
        state.embeddingFeedback = nil
        return .run { [loadEmbeddingModels] send in
          do { await send(.embeddingModelsLoaded(try await loadEmbeddingModels())) } catch {
            if !Task.isCancelled { await send(.embeddingModelsFailed(error.localizedDescription)) }
          }
        }.cancellable(id: Cancellation.embeddingLoad, cancelInFlight: true)
      case .embeddingModelsLoaded(let models):
        state.embeddingLoading = false
        state.embeddingModels = models
        return .none
      case .embeddingModelsFailed(let message):
        state.embeddingLoading = false
        state.embeddingFeedback = message
        return .none
      case .selectEmbeddingModel(let profile):
        guard !state.embeddingSwitching,
          let models = state.embeddingModels,
          models.activeProfile != profile,
          models.profiles.contains(where: { $0.profile == profile })
        else { return .none }
        state.embeddingLoading = false
        state.embeddingSwitching = true
        state.embeddingFeedback = nil
        return .merge(
          .cancel(id: Cancellation.embeddingLoad),
          .run { [selectEmbeddingModel] send in
            do {
              await send(.embeddingModelSelected(try await selectEmbeddingModel(profile)))
            } catch { await send(.embeddingSelectionFailed(error.localizedDescription)) }
          })
      case .embeddingModelSelected(let models):
        state.embeddingSwitching = false
        state.embeddingModels = models
        return .none
      case .embeddingSelectionFailed(let message):
        state.embeddingSwitching = false
        state.embeddingFeedback = message
        return .none
      case .answerLanguageChanged(let language):
        state.draft?.answerLanguage = language
        return .none
      case .answerLocaleChanged(let code):
        state.draft?.answerLocale = code
        return .none
      case .retentionChanged(let hours):
        state.draft?.diagnosticRetentionHours = hours
        return .none
      case .storageLimitChanged(let megabytes):
        state.draft?.diagnosticStorageLimitMb = megabytes
        return .none
      case .inferenceArtifactLimitChanged(let gigabytes):
        state.draft?.inferenceArtifactLimitGb = gigabytes
        return .none
      case .save:
        guard let draft = state.draft, let confirmed = state.confirmed, !state.saving else {
          return .none
        }
        state.submitted = .init(draft: draft, baseline: confirmed)
        state.feedback = nil
        return .run { [save] send in
          do {
            await send(.saved(try await save(draft)))
          } catch let error as SettingsRejection {
            await send(.saveFailed(error.message))
          } catch {
            await send(.saveFailed(error.localizedDescription))
          }
        }
      case .saved(let settings):
        guard let submitted = state.submitted else { return .none }
        let latest =
          if let confirmed = state.confirmed, confirmed.revision > settings.revision {
            confirmed
          } else {
            settings
          }
        state.rebase(on: latest, preservingEditsSince: [submitted.draft])
        state.submitted = nil
        state.feedback = "Settings saved."
        return .none
      case .saveFailed(let message):
        guard let submitted = state.submitted, let confirmed = state.confirmed else {
          return .none
        }
        state.rebase(on: confirmed, preservingEditsSince: [submitted.baseline, submitted.draft])
        state.submitted = nil
        state.feedback = message
        return .none
      }
    }
  }
}
