import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct PreferencesView: View {
  let store: StoreOf<Dashboard>

  private var form: StoreOf<SettingsForm> {
    store.scope(state: \.settingsForm, action: \.settingsForm)
  }

  var body: some View {
    WorkspacePage {
      WorkspaceHeading(title: "Settings")
      if let settings = form.draft {
        WorkspaceGroup(title: "Answers") {
          LabeledContent("Answer language") {
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
        WorkspaceGroup(title: "Diagnostics") {
          LabeledContent("Diagnostic retention") {
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
              Text("hours").foregroundStyle(.secondary)
            }
          }
          if let storageLimit = settings.diagnosticStorageLimitMb {
            Divider()
            LabeledContent("Maximum disk space") {
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
                Text("MB").foregroundStyle(.secondary)
              }
            }
          }
        }
        HStack {
          if let feedback = form.feedback { Text(feedback).foregroundStyle(.secondary) }
          Spacer()
          Button("Save") { form.send(.save) }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut("s")
            .disabled(store.connection != .ready || form.saving)
        }
      }
    }
    .labeledContentStyle(WorkspaceRowStyle())
  }
}
