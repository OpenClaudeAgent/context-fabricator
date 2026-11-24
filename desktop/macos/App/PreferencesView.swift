import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct PreferencesView: View {
  let store: StoreOf<Dashboard>
  let showModelLibrary: () -> Void
  @Binding var appLanguage: AppLanguage
  @Environment(\.locale) private var locale
  private var form: StoreOf<SettingsForm> {
    store.scope(state: \.settingsForm, action: \.settingsForm)
  }

  var body: some View {
    WorkspacePage {
      WorkspaceHeading(title: "Settings")
      general
      models
      answers
      if let settings = form.draft {
        if let limit = settings.inferenceArtifactLimitGb {
          WorkspaceGroup(title: "Inference storage") {
            LabeledContent {
              HStack(spacing: 8) {
                TextField(
                  "Maximum inference artifact disk space in GB",
                  value: Binding(
                    get: { form.draft?.inferenceArtifactLimitGb ?? limit },
                    set: { form.send(.inferenceArtifactLimitChanged($0)) }),
                  format: .number
                )
                .textFieldStyle(.roundedBorder)
                .multilineTextAlignment(.trailing)
                .monospacedDigit()
                .frame(width: 64)
                .accessibilityIdentifier("inference-artifact-limit")
              }
            } label: {
              HStack(spacing: 4) {
                Text("Maximum artifact disk space")
                Text("GB").foregroundStyle(.secondary)
              }
            }
          }
        }
        WorkspaceGroup(title: "Diagnostics") {
          LabeledContent {
            HStack(spacing: 8) {
              TextField(
                "Diagnostic retention in hours",
                value: Binding(
                  get: {
                    form.draft?.diagnosticRetentionHours ?? settings.diagnosticRetentionHours
                  },
                  set: { form.send(.retentionChanged($0)) }),
                format: .number
              )
              .textFieldStyle(.roundedBorder)
              .multilineTextAlignment(.trailing)
              .monospacedDigit()
              .frame(width: 64)
              .accessibilityIdentifier("diagnostic-retention")
            }
          } label: {
            HStack(spacing: 4) {
              Text("Diagnostic retention")
              Text("hours").foregroundStyle(.secondary)
            }
          }
          if let storageLimit = settings.diagnosticStorageLimitMb {
            Divider()
            LabeledContent {
              HStack(spacing: 8) {
                TextField(
                  "Maximum diagnostics disk space in MB",
                  value: Binding(
                    get: { form.draft?.diagnosticStorageLimitMb ?? storageLimit },
                    set: { form.send(.storageLimitChanged($0)) }),
                  format: .number
                )
                .textFieldStyle(.roundedBorder)
                .multilineTextAlignment(.trailing)
                .monospacedDigit()
                .frame(width: 64)
                .accessibilityIdentifier("diagnostic-storage-limit")
              }
            } label: {
              HStack(spacing: 4) {
                Text("Maximum disk space")
                Text("MB").foregroundStyle(.secondary)
              }
            }
          }
        }
        HStack {
          if let feedback = form.feedback {
            Text(locale.localizedKey(feedback)).foregroundStyle(.secondary)
          }
          Spacer()
          Button("Save") { form.send(.save) }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut("s")
            .disabled(store.connection != .ready || form.saving)
        }
      }
    }
    .labeledContentStyle(WorkspaceRowStyle())
    .task(id: store.connection == .ready) {
      if store.connection == .ready { await form.send(.loadEmbeddingModels).finish() }
    }
  }

  private var general: some View {
    WorkspaceGroup(title: "General") {
      LabeledContent("App Language") {
        Picker("App Language", selection: $appLanguage) {
          Text("System Default").tag(AppLanguage.system)
          ForEach(AppLanguage.allCases.filter { $0 != .system }, id: \.self) { language in
            Text(verbatim: language.name).tag(language)
          }
        }
        .labelsHidden()
        .pickerStyle(.menu)
        .fixedSize()
        .accessibilityIdentifier("app-language-picker")
      }
    }
  }

  private var answers: some View {
    WorkspaceGroup(title: "Answers") {
      VStack(alignment: .leading, spacing: 8) {
        if let settings = form.draft {
          LabeledContent("Answer language") {
            if let options = settings.answerLanguageOptions {
              AnswerLanguagePicker(
                options: options,
                selection: settings.answerLocale ?? settings.answerLanguage.rawValue
              ) { form.send(.answerLocaleChanged($0)) }
            } else {
              Picker(
                "Answer language",
                selection: Binding(
                  get: { form.draft?.answerLanguage ?? settings.answerLanguage },
                  set: { form.send(.answerLanguageChanged($0)) })
              ) {
                Text("Automatic").tag(Components.Schemas.Settings.answerLanguagePayload.auto)
                Text("English").tag(Components.Schemas.Settings.answerLanguagePayload.en)
                Text("French").tag(Components.Schemas.Settings.answerLanguagePayload.fr)
              }
              .labelsHidden()
              .fixedSize()
            }
          }
          if store.answerHistory.draft != nil || store.connection == .ready {
            Divider()
          }
        }
        AnswerHistorySettings(
          store: store.scope(state: \.answerHistory, action: \.answerHistory),
          backendReady: store.connection == .ready)
      }
    }
  }

  private var models: some View {
    WorkspaceGroup(title: "Models") {
      LabeledContent("Embedding model") {
        embeddingModelControl
      }
      Group {
        if let feedback = form.embeddingFeedback {
          Label(locale.localizedKey(feedback), systemImage: "exclamationmark.circle")
            .font(.caption).foregroundStyle(.secondary)
            .lineLimit(1).help(locale.localizedKey(feedback))
            .accessibilityIdentifier("settings-embedding-model-error")
        }
      }
      .frame(maxWidth: .infinity, minHeight: 18, maxHeight: 18, alignment: .leading)
      Divider()
      LabeledContent("Local model library") {
        Button("Manage Models…", systemImage: "slider.horizontal.3", action: showModelLibrary)
          .accessibilityIdentifier("settings-manage-models")
      }
    }
  }
  @ViewBuilder private var embeddingModelControl: some View {
    HStack(spacing: 8) {
      if form.embeddingLoading || form.embeddingSwitching {
        ProgressView().controlSize(.small)
          .accessibilityLabel("Loading embedding models…")
      }
      if let models = form.embeddingModels, !models.profiles.isEmpty {
        let activeProfile = models.profiles.first { $0.profile == models.activeProfile }?.profile
        Picker(
          "Embedding model",
          selection: Binding(
            get: { activeProfile ?? "" },
            set: { form.send(.selectEmbeddingModel($0)) }
          )
        ) {
          if activeProfile == nil {
            Text("Select an embedding model").tag("")
          }
          ForEach(models.profiles, id: \.profile) { model in
            Text(model.label).tag(model.profile)
          }
        }
        .labelsHidden()
        .pickerStyle(.menu)
        .frame(maxWidth: 260)
        .disabled(store.connection != .ready || form.embeddingLoading || form.embeddingSwitching)
        .accessibilityIdentifier("settings-embedding-model-picker")
      } else if form.embeddingModels != nil {
        Button("No embedding model installed", action: showModelLibrary)
          .accessibilityIdentifier("settings-embedding-model-empty")
      } else if form.embeddingFeedback != nil {
        Button("Retry") { form.send(.loadEmbeddingModels) }
          .disabled(store.connection != .ready)
      } else {
        Text("Loading embedding models…").foregroundStyle(.secondary)
      }
    }
    .frame(minHeight: 24, alignment: .trailing)
  }

}
