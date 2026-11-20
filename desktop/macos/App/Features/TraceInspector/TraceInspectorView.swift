import ComposableArchitecture
import DesktopFeature
import SwiftUI

struct TraceInspectorView: View {
  private enum Pane: Hashable {
    case spans
    case logs
  }

  let store: StoreOf<TraceInspector>
  let traceID: String
  let initialSpanID: String?
  @Environment(\.dismiss) private var dismiss
  @State private var pane = Pane.spans
  @State private var selectedSpan: Int64?

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header
      Divider()
      content
      footer
    }
    .frame(minWidth: 740, idealWidth: 900, minHeight: 460, idealHeight: 550)
    .task { await store.send(.load(traceID)).finish() }
    .onDisappear { store.send(.close) }
  }

  private var header: some View {
    HStack(spacing: 16) {
      VStack(alignment: .leading, spacing: 5) {
        Text("Trace").font(.title2.bold()).accessibilityAddTraits(.isHeader)
        Text(traceID).font(.caption.monospaced()).foregroundStyle(.secondary)
          .textSelection(.enabled)
          .accessibilityIdentifier("trace-inspector-id")
      }
      Spacer()
      Picker("Trace records", selection: $pane) {
        Text("Spans").tag(Pane.spans)
        Text("Logs").tag(Pane.logs)
      }
      .pickerStyle(.segmented)
      .labelsHidden()
      .fixedSize()
      .disabled(!store.hasRecords)
      .accessibilityIdentifier("trace-inspector-pane")
      Button("Refresh", systemImage: "arrow.clockwise") { store.send(.load(traceID)) }
        .disabled(store.isLoading)
      Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
    }
    .padding(20)
  }

  @ViewBuilder private var content: some View {
    if store.hasRecords {
      switch pane {
      case .spans:
        TraceSpanOutline(
          spans: store.spans, logs: store.logs, initialSpanID: initialSpanID,
          selection: $selectedSpan)
      case .logs: TraceLogsView(logs: store.logs)
      }
    } else if store.isLoading {
      ProgressView("Loading trace…").frame(maxWidth: .infinity, maxHeight: .infinity)
    } else {
      ContentUnavailableView {
        Label("Trace Unavailable", systemImage: "waveform.path")
      } description: {
        Text(store.feedback ?? "This trace may still be recording or may have expired.")
      } actions: {
        Button("Retry") { store.send(.load(traceID)) }
      }
    }
  }

  @ViewBuilder private var footer: some View {
    if store.hasRecords, store.nextBeforeId != nil || store.feedback != nil {
      Divider()
      HStack {
        if let message = store.feedback {
          Label(message, systemImage: "exclamationmark.circle").foregroundStyle(.secondary)
        }
        Spacer()
        if store.nextBeforeId != nil {
          Button("Load Older Records") { store.send(.loadMore) }
            .disabled(store.isLoading)
        }
      }
      .padding(.horizontal, 20)
      .padding(.vertical, 12)
    }
  }
}
