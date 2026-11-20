import LocalAPI
import SwiftUI

struct TraceSpanDetail: View {
  let step: TraceStep
  let logs: [Components.Schemas.SignalSample]

  var body: some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 16) {
        heading
        metadata
        spanEvents
        spanLogs
      }
      .padding(20)
      .frame(maxWidth: .infinity, alignment: .leading)
    }
    .id(step.id)
    .accessibilityIdentifier("trace-span-detail")
  }

  private var heading: some View {
    HStack(alignment: .firstTextBaseline) {
      Text(step.title)
        .font(.headline)
        .accessibilityAddTraits(.isHeader)
        .accessibilityIdentifier("trace-span-title")
      Spacer()
      Label(step.outcome.title, systemImage: step.outcome.symbol)
        .foregroundStyle(step.outcome == .failed ? Color.red : Color.secondary)
        .accessibilityIdentifier("trace-span-outcome")
    }
  }

  @ViewBuilder private var metadata: some View {
    Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
      metadataRow("Operation") {
        Text(step.sample.operation ?? "Unavailable")
          .accessibilityIdentifier("trace-span-operation")
      }
      metadataRow("Duration") {
        Text(step.sample.value.map(TraceStep.elapsed) ?? "Unavailable")
          .accessibilityIdentifier("trace-duration-\(step.sample.name)")
      }
      timestampRow
      if let spanID = step.sample.spanId {
        metadataRow("Span ID") {
          Text(spanID).accessibilityIdentifier("trace-span-id")
        }
      }
      if let parentID = step.sample.parentSpanId {
        metadataRow("Parent span ID") {
          Text(parentID).accessibilityIdentifier("trace-span-parent")
        }
      }
    }
    if step.parentMissing {
      Label("Parent span unavailable in the loaded records", systemImage: "info.circle")
        .font(.caption)
        .foregroundStyle(.secondary)
    }
  }

  private var timestampRow: some View {
    let start = step.sample.startedAtUnixMicros
    let seconds = start.map { $0 / 1_000_000 } ?? step.sample.timestamp
    let date = Date(timeIntervalSince1970: Double(seconds)).formatted(
      .dateTime.month(.abbreviated).day().hour(.twoDigits(amPM: .omitted)).minute().second())
    let timestamp = start.map { "\(date).\(String(format: "%06lld", $0 % 1_000_000))" } ?? date
    return metadataRow(start == nil ? "Recorded" : "Started") {
      Text(timestamp).accessibilityIdentifier("trace-span-start")
    }
  }

  private func metadataRow<Content: View>(
    _ title: String, @ViewBuilder value: () -> Content
  ) -> some View {
    GridRow(alignment: .firstTextBaseline) {
      Text(title).foregroundStyle(.secondary)
      value()
        .fontDesign(.monospaced)
        .textSelection(.enabled)
    }
    .font(.callout)
  }

  private var spanEvents: some View {
    VStack(alignment: .leading, spacing: 7) {
      Divider()
      Text("Span Events").font(.headline).accessibilityAddTraits(.isHeader)
      if let events = step.sample.events, !events.isEmpty {
        ForEach(Array(events.enumerated()), id: \.offset) { index, event in
          HStack(alignment: .firstTextBaseline) {
            Text(event.name).accessibilityIdentifier("trace-span-event-\(index)")
            Spacer()
            Text("+\(TraceStep.elapsed(event.offsetMicros))")
              .monospacedDigit()
              .foregroundStyle(.secondary)
              .accessibilityIdentifier("trace-span-event-offset-\(index)")
          }
        }
      } else {
        Text(
          step.sample.events == nil
            ? "Event details unavailable for this span." : "No events recorded for this span."
        )
        .foregroundStyle(.secondary)
      }
    }
  }

  private var spanLogs: some View {
    VStack(alignment: .leading, spacing: 7) {
      Divider()
      Text("Span Logs").font(.headline).accessibilityAddTraits(.isHeader)
      if logs.isEmpty {
        Text("No logs for this span in the loaded records.").foregroundStyle(.secondary)
      }
      ForEach(logs, id: \.id) { TraceLogRow(log: $0) }
    }
  }
}
