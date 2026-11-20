import LocalAPI
import SwiftUI

struct TraceSpanOutline: View {
  let spans: [Components.Schemas.SignalSample]
  let logs: [Components.Schemas.SignalSample]
  let initialSpanID: String?
  @Binding var selection: Int64?

  var body: some View {
    let steps = TraceStep.outline(spans)
    if let focus = focusedStep(in: steps) {
      HSplitView {
        outline(steps, focus: focus.id)
          .frame(minWidth: 240, idealWidth: 300, maxWidth: 380)
        TraceSpanDetail(step: focus, logs: relatedLogs(to: focus))
          .frame(minWidth: 440, maxWidth: .infinity)
      }
    } else {
      ContentUnavailableView(
        "No Spans in the Loaded Records", systemImage: "point.3.connected.trianglepath.dotted")
    }
  }

  private func outline(_ steps: [TraceStep], focus: Int64) -> some View {
    List(selection: Binding(get: { focus }, set: { selection = $0 })) {
      Section("Span hierarchy") {
        ForEach(steps) { step in
          HStack(spacing: 6) {
            if step.depth > 0 {
              Image(systemName: "arrow.turn.down.right")
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            }
            Text(step.title)
              .lineLimit(2)
              .accessibilityIdentifier("trace-step-\(step.sample.name)")
            Spacer(minLength: 6)
            if let micros = step.sample.value {
              Text(TraceStep.elapsed(micros))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            }
            Image(systemName: step.outcome.symbol)
              .foregroundStyle(step.outcome == .failed ? Color.red : Color.secondary)
              .accessibilityLabel(step.outcome.title)
          }
          .padding(.leading, CGFloat(min(step.depth, 4) * 11))
          .padding(.vertical, 4)
          .contentShape(Rectangle())
          .tag(step.id)
        }
      }
    }
    .listStyle(.inset)
  }

  private func focusedStep(in steps: [TraceStep]) -> TraceStep? {
    steps.first { $0.id == selection }
      ?? initialSpanID.flatMap { target in steps.first { $0.sample.spanId == target } }
      ?? steps.first
  }

  private func relatedLogs(to step: TraceStep) -> [Components.Schemas.SignalSample] {
    guard let spanID = step.sample.spanId else { return [] }
    return logs.filter { $0.spanId == spanID }
  }
}
