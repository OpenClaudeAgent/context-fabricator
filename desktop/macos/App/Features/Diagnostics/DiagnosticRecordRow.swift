import Foundation
import LocalAPI
import SwiftUI

struct DiagnosticsSampleRow: View {
  @Environment(\.locale) private var locale
  enum Kind {
    case logs
    case traces
  }

  let value: Components.Schemas.SignalSample
  let kind: Kind
  let showTrace: (String, String?) -> Void

  var body: some View {
    DiagnosticRecordLayout(timestamp: value.timestamp) {
      record
        .frame(maxWidth: .infinity, alignment: .leading)
      traceAction
        .frame(width: 18)
    }
  }

  @ViewBuilder private var traceAction: some View {
    if let trace = value.traceId {
      Button("View Trace", systemImage: "waveform.path") { showTrace(trace, value.spanId) }
        .labelStyle(.iconOnly)
        .buttonStyle(.borderless)
        .controlSize(.small)
        .help("View Trace")
        .accessibilityLabel("View Trace")
        .accessibilityIdentifier("view-\(kind == .logs ? "log" : "span")-trace-\(value.id)")
    } else {
      Color.clear.frame(height: 1).accessibilityHidden(true)
    }
  }

  @ViewBuilder private var record: some View {
    if value.traceId != nil || value.spanId != nil || value.parentSpanId != nil {
      DisclosureGroup {
        VStack(alignment: .leading, spacing: 5) {
          if let trace = value.traceId { traceValue("Trace ID", value: trace) }
          if let span = value.spanId { traceValue("Span ID", value: span) }
          if let parent = value.parentSpanId { traceValue("Parent span", value: parent) }
        }
        .padding(.vertical, 6)
      } label: {
        sampleSummary
      }
      .accessibilityIdentifier(recordIdentifier)
    } else {
      sampleSummary.accessibilityIdentifier(recordIdentifier)
    }
  }

  private var recordIdentifier: String {
    kind == .traces ? "telemetry-span" : "telemetry-log-\(value.name)"
  }

  private var sampleSummary: some View {
    HStack(alignment: .firstTextBaseline, spacing: 8) {
      VStack(alignment: .leading, spacing: 3) {
        Text(summaryTitle)
          .font(.callout.weight(.medium))
        if let operation = summaryOperation {
          Text(operation).font(.caption.monospaced()).foregroundStyle(.secondary)
        }
        if kind == .logs, let problem = value.problemCode {
          Text(problem).font(.caption.monospaced()).foregroundStyle(.secondary)
        }
      }
      Spacer(minLength: 8)
      if let duration = value.value {
        Text(TraceStep.elapsed(duration))
          .font(.callout)
          .monospacedDigit()
      }
      status
        .frame(width: kind == .traces ? 128 : 54, alignment: .trailing)
        .lineLimit(1)
      if let status = value.httpStatus {
        Text("HTTP \(status)")
          .font(.caption.monospacedDigit())
          .foregroundStyle(.secondary)
          .frame(width: 55, alignment: .trailing)
      } else {
        Color.clear.frame(width: 55, height: 1).accessibilityHidden(true)
      }
    }
  }

  @ViewBuilder private var status: some View {
    if kind == .traces {
      let outcome = value.spanOutcome ?? .unknown
      Label(outcome.title(in: locale), systemImage: outcome.symbol)
        .font(.caption)
        .foregroundStyle(outcome.tint)
        .help(outcome.title(in: locale))
    } else if let level = value.level {
      severity(level)
    } else {
      Color.clear.frame(height: 1).accessibilityHidden(true)
    }
  }

  private var summaryTitle: String {
    TraceStep.title(for: value, in: locale)
  }

  private var summaryOperation: String? {
    let operation =
      if let method = value.httpMethod, let route = value.httpRoute {
        "\(method) \(route)"
      } else if kind == .logs, value.name == "api_request" {
        value.httpMethod
      } else {
        value.operation
      }
    return operation == summaryTitle ? nil : operation
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
      Text(level.localizedTitle).foregroundStyle(.secondary)
    } icon: {
      Image(systemName: symbol).foregroundStyle(tint)
    }
    .font(.caption)
  }

  private func traceValue(_ title: LocalizedStringKey, value: String) -> some View {
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
      .lineLimit(1)
      .frame(width: 140, alignment: .leading)
      content
    }
    .padding(.vertical, 4)
  }

}
