import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct DiagnosticLogsView: View {
  let store: StoreOf<DiagnosticLogs>
  let events: [Components.Schemas.Diagnostic]
  let ready: Bool
  let showTrace: (String, String?) -> Void

  var body: some View {
    let visibleEvents = store.level == nil ? events.filter { matches($0) } : []
    return VStack(spacing: 0) {
      filterBar(hasRecords: !store.records.isEmpty || !visibleEvents.isEmpty)
      Divider()
      logList(visibleEvents: visibleEvents)
    }
  }

  private func filterBar(hasRecords: Bool) -> some View {
    HStack(spacing: 10) {
      if hasRecords && store.isLoading {
        ProgressView().controlSize(.small).accessibilityLabel("Loading logs")
      }
      levelPicker
    }
    .frame(maxWidth: .infinity, alignment: .trailing)
    .padding(.horizontal, 18)
    .padding(.vertical, 10)
  }

  private var levelPicker: some View {
    HStack(spacing: 6) {
      Text("Level").foregroundStyle(.secondary).accessibilityHidden(true)
      Picker(
        "Log level",
        selection: Binding(get: { store.level }, set: { store.send(.levelChanged($0)) })
      ) {
        Text("All Levels").tag(nil as Components.Schemas.DiagnosticLogLevel?)
        Text("Info").tag(Components.Schemas.DiagnosticLogLevel.info)
        Text("Error").tag(Components.Schemas.DiagnosticLogLevel.error)
        Text("Debug").tag(Components.Schemas.DiagnosticLogLevel.debug)
      }
      .pickerStyle(.menu)
      .labelsHidden()
      .accessibilityLabel("Log level")
      .accessibilityIdentifier("diagnostic-log-level")
      .disabled(!ready)
      .fixedSize()
    }
    .font(.callout)
  }

  private func logList(visibleEvents: [Components.Schemas.Diagnostic]) -> some View {
    List {
      if let feedback = store.feedback, !store.records.isEmpty || !visibleEvents.isEmpty {
        Label(feedback, systemImage: "exclamationmark.circle").foregroundStyle(.secondary)
      }
      if !visibleEvents.isEmpty {
        Section("Backend Events") {
          ForEach(visibleEvents, id: \.id) { event in
            DiagnosticRecordLayout(timestamp: event.timestamp) {
              VStack(alignment: .leading, spacing: 3) {
                Text(event.message)
                Text(event.code).font(.caption.monospaced()).foregroundStyle(.secondary)
              }
            }
          }
        }
      }
      if !store.records.isEmpty || store.nextBeforeId != nil {
        Section("Telemetry Logs") {
          ForEach(store.records, id: \.id) { sample in
            DiagnosticsSampleRow(value: sample, kind: .logs, showTrace: showTrace)
          }
          if store.nextBeforeId != nil {
            HStack {
              Spacer(minLength: 0)
              Button("Load more logs") { store.send(.loadMore) }
                .disabled(!ready || store.isLoading)
                .accessibilityIdentifier("diagnostic-logs-load-more")
            }
          }
        }
      }
    }
    .listStyle(.inset)
    .scrollContentBackground(.hidden)
    .overlay {
      if store.records.isEmpty && visibleEvents.isEmpty {
        emptyState
      }
    }
  }

  @ViewBuilder private var emptyState: some View {
    if store.isLoading {
      LoadingStateView("Loading logs…")
    } else if let message = store.feedback {
      ContentUnavailableView(
        "Couldn’t Load Logs", systemImage: "exclamationmark.circle",
        description: Text(message))
    } else if !ready {
      ContentUnavailableView("Logs Unavailable", systemImage: "waveform.path")
    } else if !store.query.isEmpty {
      ContentUnavailableView.search(text: store.query)
        .accessibilityIdentifier("diagnostic-logs-no-results")
    } else {
      ContentUnavailableView(
        store.level == nil ? "No Logs Yet" : "No Logs at This Level", systemImage: "waveform.path")
    }
  }

  private func matches(_ event: Components.Schemas.Diagnostic) -> Bool {
    let query = store.query.trimmingCharacters(in: .whitespacesAndNewlines)
    return query.isEmpty || event.message.localizedCaseInsensitiveContains(query)
      || event.code.localizedCaseInsensitiveContains(query)
  }
}
