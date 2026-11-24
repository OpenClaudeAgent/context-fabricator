import LocalAPI
import SwiftUI

struct TraceLogsView: View {
  let logs: [Components.Schemas.SignalSample]

  var body: some View {
    if logs.isEmpty {
      ContentUnavailableView("No Logs in the Loaded Records", systemImage: "list.bullet.rectangle")
    } else {
      List(logs, id: \.id) { log in
        VStack(alignment: .leading, spacing: 5) {
          TraceLogRow(log: log)
          if let spanID = log.spanId {
            Text("Span \(spanID)")
              .font(.caption.monospaced())
              .foregroundStyle(.secondary)
              .textSelection(.enabled)
          }
        }
        .padding(.vertical, 3)
      }
      .listStyle(.inset)
    }
  }
}

struct TraceLogRow: View {
  @Environment(\.locale) private var locale
  let log: Components.Schemas.SignalSample

  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: 12) {
      Text(TraceStep.title(for: log, in: locale))
        .accessibilityIdentifier("trace-log-\(log.name)")
      Spacer()
      Text(log.level?.localizedTitle ?? "Unknown")
        .font(.caption)
        .foregroundStyle(.secondary)
        .frame(width: 55, alignment: .trailing)
      Text(
        Date(timeIntervalSince1970: TimeInterval(log.timestamp)),
        format: .dateTime.hour().minute().second()
      )
      .font(.caption)
      .monospacedDigit()
      .foregroundStyle(.secondary)
      .frame(minWidth: 58, alignment: .trailing)
    }
  }
}
