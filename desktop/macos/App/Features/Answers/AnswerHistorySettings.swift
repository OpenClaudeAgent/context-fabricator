import ComposableArchitecture
import DesktopFeature
import SwiftUI

struct AnswerHistorySettings: View {
  @Environment(\.locale) private var locale
  private enum Retention: Int, CaseIterable {
    case day = 1
    case week = 7
    case month = 30
    case quarter = 90
    case year = 365
    case custom = 0

    var title: LocalizedStringKey {
      switch self {
      case .day: "1 day"
      case .week: "7 days"
      case .month: "30 days"
      case .quarter: "90 days"
      case .year: "1 year"
      case .custom: "Custom…"
      }
    }
  }

  let store: StoreOf<AnswerHistory>
  let backendReady: Bool
  @State private var editsCustomRetention = false
  @State private var customDraft: String?
  @FocusState private var customFieldFocused: Bool
  @State private var confirmClear = false
  @State private var confirmDisable = false

  private var validCustomDays: Int? {
    guard let customDraft, let value = Int(customDraft), (1...365).contains(value) else {
      return nil
    }
    return value
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      if let settings = store.draft {
        LabeledContent("Save answer history") {
          Toggle(
            "Save answer history",
            isOn: Binding(
              get: { store.draft?.enabled ?? settings.enabled },
              set: { enabled in
                if enabled {
                  store.send(.enabledChanged(true))
                } else {
                  confirmDisable = true
                }
              })
          )
          .toggleStyle(.switch)
          .labelsHidden()
          .accessibilityIdentifier("answer-history-enabled")
        }
        Divider()
        retentionFields(days: settings.retentionDays)
        Divider()
        actions
      } else if let feedback = store.settingsFeedback {
        Label(locale.localizedKey(feedback), systemImage: "exclamationmark.circle")
          .foregroundStyle(.secondary)
      } else if backendReady {
        LoadingStateView("Loading history settings").padding(.vertical, 8)
      }
    }
    .disabled(!backendReady || store.clearing)
    .task(id: backendReady) {
      if backendReady { await store.send(.loadSettings).finish() }
    }
    .onDisappear { commitCustomRetention() }
    .confirmationDialog("Clear all saved answers?", isPresented: $confirmClear) {
      Button("Clear History", role: .destructive) { store.send(.clear) }
    } message: {
      Text(
        "This clears saved answers and recent session questions, including history for the current request."
      )
    }
    .confirmationDialog("Disable and clear answer history?", isPresented: $confirmDisable) {
      Button("Disable and Clear", role: .destructive) { store.send(.enabledChanged(false)) }
    } message: {
      Text("This deletes saved questions and answers and turns off history.")
    }
  }

  private func retentionFields(days: Int) -> some View {
    let current = editsCustomRetention ? .custom : (Retention(rawValue: days) ?? .custom)
    return Group {
      LabeledContent("Keep answers for") {
        Picker(
          "Keep answers for",
          selection: Binding(
            get: { current },
            set: { option in
              customDraft = nil
              editsCustomRetention = option == .custom
              if editsCustomRetention {
                customFieldFocused = true
              } else {
                store.send(.retentionChanged(option.rawValue))
              }
            })
        ) {
          ForEach(Retention.allCases, id: \.self) { option in
            Text(option.title).tag(option)
          }
        }
        .labelsHidden()
        .pickerStyle(.menu)
        .fixedSize()
        .accessibilityIdentifier("answer-history-retention")
      }
      if current == .custom {
        Divider()
        customDuration(days: days)
      }
    }
  }

  private func customDuration(days: Int) -> some View {
    LabeledContent("Custom duration") {
      HStack(spacing: 8) {
        TextField(
          "Retention in days",
          text: Binding(get: { customDraft ?? String(days) }, set: { customDraft = $0 })
        )
        .textFieldStyle(.roundedBorder)
        .multilineTextAlignment(.trailing)
        .frame(width: 64)
        .focused($customFieldFocused)
        .onSubmit(commitCustomRetention)
        .onChange(of: customFieldFocused) { _, focused in
          if !focused { commitCustomRetention() }
        }
        .accessibilityIdentifier("answer-history-custom-days")
        Text("days").foregroundStyle(.secondary)
      }
    }
  }

  private func commitCustomRetention() {
    guard let days = validCustomDays else { return }
    customDraft = nil
    store.send(.retentionChanged(days))
  }

  private var status: String? {
    if customDraft != nil && validCustomDays == nil {
      return locale.localized("Enter a duration from 1 to 365 days.")
    }
    if store.saving { return locale.localized("Saving…") }
    return store.settingsFeedback.map { locale.localizedKey($0) }
  }

  private var actions: some View {
    HStack(spacing: 12) {
      if let status {
        Text(status)
          .font(.callout)
          .foregroundStyle(.secondary)
          .lineLimit(1)
          .help(status)
      }
      Spacer(minLength: 0)
      Button("Clear History…", role: .destructive) { confirmClear = true }
        .disabled(store.saving)
        .fixedSize()
        .accessibilityIdentifier("clear-answer-history")
    }
  }
}
