import LocalAPI
import SwiftUI

struct TraceSpanOutline: View {
  @Environment(\.locale) private var locale
  let spans: [Components.Schemas.SignalSample]
  let logs: [Components.Schemas.SignalSample]
  let initialSpanID: String?
  @Binding var selection: Int64?
  let viewCollectionTrace: (String) -> Void

  var body: some View {
    let steps = TraceStep.outline(spans)
    if let focus = focusedStep(in: steps) {
      HSplitView {
        outline(steps, focus: focus.id)
          .frame(minWidth: 205, idealWidth: 245, maxWidth: 300)
        TraceSpanDetail(
          step: focus, logs: relatedLogs(to: focus), viewCollectionTrace: viewCollectionTrace
        )
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
            Text(step.title(in: locale))
              .lineLimit(2)
              .accessibilityIdentifier("trace-step-\(step.sample.name)")
            Spacer(minLength: 6)
            if let micros = step.sample.value {
              Text(TraceStep.elapsed(micros))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            }
            Image(systemName: step.outcome.symbol)
              .foregroundStyle(tint(step.outcome))
              .accessibilityLabel(step.outcome.title(in: locale))
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

  private func tint(_ outcome: TraceStep.Outcome) -> Color {
    outcome == .succeeded ? .secondary : outcome.tint
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
