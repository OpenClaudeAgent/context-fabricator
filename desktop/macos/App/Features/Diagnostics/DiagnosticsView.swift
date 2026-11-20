import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct DiagnosticsView: View {
  let store: StoreOf<Dashboard>
  let showTrace: (String, String?) -> Void
  @State private var filter = ""

  private var telemetry: Components.Schemas.TelemetrySnapshot? { store.snapshot?.telemetry }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header
        .padding(.horizontal, 24)
        .padding(.top, 20)
        .padding(.bottom, 14)
      tabs
        .padding(.horizontal, 16)
        .padding(.bottom, 12)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .toolbar {
      if let data = telemetry.flatMap({ try? JSONEncoder().encode($0) }) {
        ToolbarItem(placement: .primaryAction) {
          ShareLink(item: String(decoding: data, as: UTF8.self)) {
            Label("Export diagnostics", systemImage: "square.and.arrow.up")
          }
        }
      }
    }
  }

  @ViewBuilder private var tabs: some View {
    let logs = store.diagnosticTab == .logs
    if store.diagnosticTab == .storage || (logs && store.connection != .ready) {
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
    TabView(
      selection: Binding(
        get: { store.diagnosticTab }, set: { store.send(.diagnosticTabChanged($0)) })
    ) {
      ForEach(Dashboard.DiagnosticTab.allCases, id: \.self) { tab in
        signalContent(tab)
          .tabItem { Text(tab.rawValue) }
          .tag(tab)
      }
    }
  }

  @ViewBuilder private func signalContent(_ tab: Dashboard.DiagnosticTab) -> some View {
    if tab == .storage {
      if let telemetry {
        DiagnosticsStorageView(
          health: telemetry.health,
          commandsEnabled: !store.telemetryBusy && store.connection == .ready
        ) { store.send(.telemetryCommand(.purgeExpired)) }
      } else {
        ContentUnavailableView("Storage Unavailable", systemImage: "internaldrive")
      }
    } else if tab == .logs {
      DiagnosticLogsView(
        store: store.scope(state: \.diagnosticLogs, action: \.diagnosticLogs),
        events: store.snapshot?.diagnostics ?? [], ready: store.connection == .ready,
        showTrace: showTrace)
    } else {
      signalList(tab)
    }
  }

  private var header: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(alignment: .firstTextBaseline, spacing: 12) {
        WorkspaceHeading(title: "Diagnostics")
        if store.connection == .ready, let telemetry { signalHealth(telemetry.health) }
        debugControl
      }
      if case .unavailable(let message) = store.connection {
        Label(message, systemImage: "exclamationmark.circle")
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
    let status: String
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

  private func signalList(_ tab: Dashboard.DiagnosticTab) -> some View {
    let values = samples(for: tab).filter {
      matchesQuery(
        filter, $0.name, $0.displayName ?? "", $0.operation ?? "", $0.traceId ?? "", $0.spanId ?? ""
      )
    }
    return List {
      if !values.isEmpty {
        Section(tab == .metrics ? "Operation Counts" : "Recorded Spans") {
          ForEach(values, id: \.id) { sample in
            DiagnosticsSampleRow(
              value: sample, tab: tab, showTrace: tab == .traces ? showTrace : nil)
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
            telemetry == nil ? "Diagnostics Unavailable" : "No \(tab.rawValue) Yet",
            systemImage: "waveform.path")
        } else {
          ContentUnavailableView.search(text: filter)
        }
      }
    }
  }

  private func samples(for tab: Dashboard.DiagnosticTab) -> [Components.Schemas.SignalSample] {
    switch tab {
    case .metrics: telemetry?.metrics ?? []
    case .traces: telemetry?.traces ?? []
    case .logs, .storage: []
    }
  }

  private func matchesQuery(_ query: String, _ values: String...) -> Bool {
    query.isEmpty || values.contains { $0.localizedCaseInsensitiveContains(query) }
  }

}
