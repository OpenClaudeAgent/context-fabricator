import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct DiagnosticsSampleRow: View {
  let value: Components.Schemas.SignalSample
  let tab: Dashboard.DiagnosticTab
  let showTrace: ((String, String?) -> Void)?

  var body: some View {
    HStack(alignment: .top, spacing: 8) {
      record
      if let trace = value.traceId, let showTrace {
        Button("View Trace", systemImage: "waveform.path") { showTrace(trace, value.spanId) }
          .labelStyle(.iconOnly)
          .buttonStyle(.borderless)
          .controlSize(.small)
          .padding(.top, 4)
          .help("View Trace")
          .accessibilityLabel("View Trace")
          .accessibilityIdentifier("view-\(tab == .logs ? "log" : "span")-trace-\(value.id)")
      }
    }
  }

  @ViewBuilder private var record: some View {
    if tab == .traces || tab == .logs,
      value.traceId != nil || value.spanId != nil || value.parentSpanId != nil
    {
      DisclosureGroup {
        VStack(alignment: .leading, spacing: 5) {
          if let trace = value.traceId { traceValue("Trace ID", value: trace) }
          if let span = value.spanId { traceValue("Span ID", value: span) }
          if let parent = value.parentSpanId { traceValue("Parent span", value: parent) }
        }
        .padding(.vertical, 6)
      } label: {
        sampleSummary(value)
      }
      .accessibilityIdentifier(tab == .traces ? "telemetry-span" : "telemetry-log-\(value.name)")
    } else {
      sampleSummary(value)
        .accessibilityIdentifier(
          tab == .metrics ? "telemetry-metric" : "telemetry-log-\(value.name)")
    }
  }

  private func sampleSummary(_ value: Components.Schemas.SignalSample) -> some View {
    DiagnosticRecordLayout(timestamp: value.timestamp) {
      Text(value.displayName ?? value.name).font(.callout.weight(.medium))
      Spacer(minLength: 8)
      if let number = value.value {
        Text(tab == .traces ? "\(number.formatted()) µs" : number.formatted())
          .monospacedDigit()
      }
      if let level = value.level {
        severity(level)
      }
    }
  }

  private func severity(_ level: Components.Schemas.SignalSample.levelPayload) -> some View {
    let symbol: String
    let tint: Color
    switch level {
    case .ok: (symbol, tint) = ("checkmark.circle", .green)
    case .info: (symbol, tint) = ("info.circle", .secondary)
    case .error: (symbol, tint) = ("exclamationmark.circle", .red)
    case .debug: (symbol, tint) = ("ladybug", .blue)
    }
    return Label {
      Text(level.rawValue.capitalized).foregroundStyle(.secondary)
    } icon: {
      Image(systemName: symbol).foregroundStyle(tint)
    }
    .font(.caption)
  }

  private func traceValue(_ title: String, value: String) -> some View {
    LabeledContent(title) {
      Text(value).font(.caption.monospaced()).textSelection(.enabled)
    }
  }

}

struct DiagnosticRecordLayout<Content: View>: View {
  let timestamp: Int64
  @ViewBuilder let content: Content

  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: 16) {
      Text(
        Date(timeIntervalSince1970: TimeInterval(timestamp)),
        format: .dateTime.month().day().hour().minute().second()
      )
      .monospacedDigit()
      .foregroundStyle(.secondary)
      .frame(minWidth: 140, alignment: .leading)
      content
    }
    .padding(.vertical, 4)
  }

}
