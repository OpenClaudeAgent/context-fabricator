import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct DiagnosticsView: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<Dashboard>
  let showTrace: (String, String?) -> Void
  @State private var filter = ""
  @State private var showsTraceLookup = false
  @State private var includesBackgroundChecks = false
  @State private var traceInput = ""
  @State private var pendingTraceID: String?

  private var telemetry: Components.Schemas.TelemetrySnapshot? { store.snapshot?.telemetry }
  private var traceIDForLookup: String {
    traceInput.trimmingCharacters(in: .whitespacesAndNewlines)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header
        .padding(.horizontal, 24)
        .padding(.top, 20)
        .padding(.bottom, 14)
      tabs
        .padding(.horizontal, 24)
        .padding(.bottom, 12)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .toolbar {
      ToolbarItem(placement: .primaryAction) {
        Button("Open Trace…", systemImage: "waveform.path") {
          traceInput = ""
          showsTraceLookup = true
        }
        .disabled(store.connection != .ready)
        .accessibilityIdentifier("open-diagnostic-trace")
      }
      if let data = telemetry.flatMap({ try? JSONEncoder().encode($0) }) {
        ToolbarItem(placement: .primaryAction) {
          ShareLink(item: String(decoding: data, as: UTF8.self)) {
            Label("Export diagnostics", systemImage: "square.and.arrow.up")
          }
        }
      }
    }
    .sheet(isPresented: $showsTraceLookup, onDismiss: openPendingTrace) { traceLookup }
  }

  private var traceLookup: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text("Open Trace").font(.headline).accessibilityAddTraits(.isHeader)
      TextField("Trace ID", text: $traceInput)
        .textFieldStyle(.roundedBorder)
        .autocorrectionDisabled()
        .accessibilityIdentifier("diagnostic-trace-id")
      HStack {
        Spacer()
        Button("Cancel") { showsTraceLookup = false }.keyboardShortcut(.cancelAction)
        Button("Open") {
          pendingTraceID = traceIDForLookup
          showsTraceLookup = false
        }
        .keyboardShortcut(.defaultAction)
        .disabled(traceIDForLookup.isEmpty)
        .accessibilityIdentifier("diagnostic-open-trace")
      }
    }
    .padding(16)
    .frame(width: 320)
  }

  private func openPendingTrace() {
    guard let id = pendingTraceID else { return }
    pendingTraceID = nil
    showTrace(id, nil)
  }

  @ViewBuilder private var tabs: some View {
    let logs = store.diagnosticTab == .logs
    if store.diagnosticTab == .requests || store.diagnosticTab == .storage
      || store.diagnosticTab == .answerUsage
      || (logs && store.connection != .ready)
    {
      diagnosticTabs
    } else {
      diagnosticTabs.searchable(
        text: logs ? logSearch : $filter, placement: .toolbar,
        prompt: logs ? "Search logs" : "Filter diagnostics")
    }
  }

  private var logSearch: Binding<String> {
    Binding(
      get: { store.diagnosticLogs.query },
      set: { store.send(.diagnosticLogs(.queryChanged($0))) })
  }

  private var diagnosticTabs: some View {
    VStack(alignment: .leading, spacing: 16) {
      Picker(
        "Diagnostics section",
        selection: Binding(
          get: { store.diagnosticTab }, set: { store.send(.diagnosticTabChanged($0)) })
      ) {
        ForEach(Dashboard.DiagnosticTab.allCases, id: \.self) { tab in
          Text(tab.localizedTitle).tag(tab)
        }
      }
      .pickerStyle(.segmented).labelsHidden()
      .frame(maxWidth: .infinity, alignment: .leading)
      .accessibilityIdentifier("diagnostic-tabs")
      signalContent(store.diagnosticTab)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
  }

  @ViewBuilder private func signalContent(_ tab: Dashboard.DiagnosticTab) -> some View {
    if tab == .requests {
      if store.connection == .ready {
        DiagnosticAnswerRequestsView(
          store: store.scope(state: \.answerRequests, action: \.answerRequests),
          answers: store.scope(state: \.answers, action: \.answers),
          openTrace: { showTrace($0, nil) })
      } else {
        ContentUnavailableView(
          "Requests Unavailable", systemImage: "bubble.left.and.text.bubble.right"
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
      }
    } else if tab == .answerUsage {
      AnswerUsageView(
        store: store.scope(state: \.answerUsage, action: \.answerUsage),
        ready: store.connection == .ready, showTrace: showTrace)
    } else if tab == .metrics {
      DiagnosticMetricsView(snapshot: telemetry, query: filter)
    } else if tab == .storage {
      if let telemetry {
        DiagnosticsStorageView(
          health: telemetry.health,
          commandsEnabled: !store.telemetryBusy && store.connection == .ready
        ) { store.send(.telemetryCommand(.purgeExpired)) }
      } else {
        ContentUnavailableView("Storage Unavailable", systemImage: "internaldrive")
          .frame(maxWidth: .infinity, maxHeight: .infinity)
      }
    } else if tab == .logs {
      DiagnosticLogsView(
        store: store.scope(state: \.diagnosticLogs, action: \.diagnosticLogs),
        events: store.snapshot?.diagnostics ?? [], ready: store.connection == .ready,
        showTrace: showTrace)
    } else {
      VStack(spacing: 0) {
        Toggle("Include background checks", isOn: $includesBackgroundChecks)
          .toggleStyle(.checkbox)
          .font(.callout)
          .frame(maxWidth: .infinity, alignment: .trailing)
          .padding(.horizontal, 18)
          .padding(.vertical, 10)
          .accessibilityIdentifier("include-background-traces")
        Divider()
        traceList
      }
      .task(id: store.answers.answerTraceID ?? "") { await store.send(.readTelemetry).finish() }
    }
  }

  private var header: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(alignment: .firstTextBaseline, spacing: 12) {
        WorkspaceHeading(title: "Diagnostics")
        if store.connection == .ready, let telemetry { signalHealth(telemetry.health) }
        debugControl
      }
      if let message = store.connection.failureMessage {
        Label(locale.localizedKey(message), systemImage: "exclamationmark.circle")
          .foregroundStyle(.secondary)
          .accessibilityIdentifier("diagnostics-launcher-failure")
      }
      if let feedback = store.telemetryFeedback {
        Label(feedback, systemImage: "info.circle").foregroundStyle(.secondary)
      }
    }
  }

  @ViewBuilder private var debugControl: some View {
    if let telemetry {
      Group {
        if telemetry.health.debugUntil == 0 {
          Button("Debug for 5 minutes", systemImage: "ladybug") {
            store.send(.telemetryCommand(.enableDebug))
          }
        } else {
          Button("Disable debug", systemImage: "stop.circle") {
            store.send(.telemetryCommand(.disableDebug))
          }
        }
      }
      .disabled(store.telemetryBusy || store.connection != .ready)
      .fixedSize()
    }
  }

  private func signalHealth(_ health: Components.Schemas.TelemetryHealth) -> some View {
    let status: LocalizedStringKey
    let symbol: String
    let tint: Color
    switch health.state {
    case .ready:
      (status, symbol, tint) = ("Recording", "checkmark.circle", .green)
    case .degraded:
      (status, symbol, tint) = ("Degraded", "exclamationmark.circle", .orange)
    case .gaps:
      (status, symbol, tint) = ("Data gaps", "exclamationmark.triangle", .orange)
    }
    return Label {
      Text(status)
    } icon: {
      Image(systemName: symbol).foregroundStyle(tint)
    }
    .font(.callout)
    .accessibilityIdentifier("diagnostic-recording-status")
  }

  private var traceList: some View {
    let values = (telemetry?.traces ?? []).filter {
      (includesBackgroundChecks || $0.traceRequestKind != .backgroundCheck)
        && matchesQuery(
          filter, $0.name, $0.displayName ?? "", $0.operation ?? "", $0.httpRoute ?? "",
          $0.traceId ?? "", $0.spanId ?? ""
        )
    }
    return List {
      if !values.isEmpty {
        Section("Recorded Spans") {
          ForEach(values, id: \.id) { sample in
            DiagnosticsSampleRow(value: sample, kind: .traces, showTrace: showTrace)
          }
        }
      }
    }
    .listStyle(.inset)
    .scrollContentBackground(.hidden)
    .overlay {
      if values.isEmpty {
        if filter.isEmpty {
          ContentUnavailableView(
            telemetry == nil ? "Diagnostics Unavailable" : "No Traces Yet",
            systemImage: "waveform.path")
        } else {
          ContentUnavailableView.search(text: filter)
        }
      }
    }
  }

  private func matchesQuery(_ query: String, _ values: String...) -> Bool {
    query.isEmpty || values.contains { $0.localizedCaseInsensitiveContains(query) }
  }

}
