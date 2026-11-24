import LocalAPI
import SwiftUI

struct TraceSpanDetail: View {
  @Environment(\.locale) private var locale
  let step: TraceStep
  let logs: [Components.Schemas.SignalSample]
  let viewCollectionTrace: (String) -> Void

  var body: some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 16) {
        heading
        if let budget = step.sample.contextBudget {
          contextBudget(budget)
        }
        metadata
        if let traceID = step.sample.workTraceId, !traceID.isEmpty {
          Button("View collection trace", systemImage: "arrow.right.circle") {
            viewCollectionTrace(traceID)
          }
          .accessibilityIdentifier("trace-view-collection-trace")
        }
        if let timing = step.sample.inferenceTiming {
          inferenceTiming(timing)
        }
        if let ranking = step.sample.fusionRanking {
          fusionRanking(ranking)
        }
        if let counts = step.sample.retrievalCounts {
          TraceRetrievalCounts(counts: counts)
        }
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
      Text(step.title(in: locale))
        .font(.headline)
        .accessibilityAddTraits(.isHeader)
        .accessibilityIdentifier("trace-span-title")
      Spacer()
      Label(step.outcome.title(in: locale), systemImage: step.outcome.symbol)
        .foregroundStyle(step.outcome.tint)
        .accessibilityIdentifier("trace-span-outcome")
    }
  }

  @ViewBuilder private var metadata: some View {
    Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
      metadataRow("Operation") {
        Text(step.sample.operation ?? locale.localized("Unavailable"))
          .accessibilityIdentifier("trace-span-operation")
      }
      if let method = step.sample.httpMethod {
        metadataRow("HTTP request") {
          Text("\(method) \(step.sample.httpRoute ?? locale.localized("Unmatched route"))")
            .accessibilityIdentifier("trace-span-http-request")
        }
      }
      metadataRow("Duration") {
        Text(step.sample.value.map(TraceStep.elapsed) ?? locale.localized("Unavailable"))
          .accessibilityIdentifier("trace-duration-\(step.sample.name)")
      }
      if let status = step.sample.httpStatus {
        metadataRow("HTTP response") {
          Text(status.formatted()).accessibilityIdentifier("trace-span-http-status")
        }
      }
      if let code = step.sample.problemCode {
        metadataRow("Problem code") {
          Text(code).accessibilityIdentifier("trace-span-problem-code")
        }
      }
      if let client = step.sample.clientKind {
        metadataRow("Client") {
          Text(client == .desktop ? "macOS" : client == .cli ? "CLI" : "Other / untagged")
            .accessibilityIdentifier("trace-span-client")
        }
      }
      if let model = step.sample.modelProfile {
        metadataRow("Model") {
          Text(model).textSelection(.enabled).accessibilityIdentifier("trace-span-model")
        }
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

  private func contextBudget(_ budget: Components.Schemas.GenerationContext) -> some View {
    let summary: LocalizedStringKey =
      switch budget.kind {
      case .passages: "\(budget.admitted) of \(budget.proposed) passages selected"
      case .linkedPassages: "\(budget.admitted) of \(budget.proposed) linked passages selected"
      case .historyPairs: "\(budget.admitted) of \(budget.proposed) history pairs selected"
      case .workflowAttempt: "\(budget.admitted) of \(budget.proposed) workflow attempts selected"
      }
    return VStack(alignment: .leading, spacing: 5) {
      Label("Model Context", systemImage: "doc.text.magnifyingglass")
        .font(.caption).foregroundStyle(.secondary)
        .accessibilityAddTraits(.isHeader)
      Text(summary)
        .font(.headline)
        .accessibilityIdentifier("trace-context-admitted")
      if let input = budget.inputTokens {
        Text("\(input.formatted()) of \(budget.inputTokenLimit.formatted()) input tokens")
          .monospacedDigit()
      } else {
        Text("No evidence fit within the \(budget.inputTokenLimit.formatted())-token limit.")
          .accessibilityIdentifier("trace-context-no-fit")
      }
      LabeledContent("Token checks", value: budget.measurements.formatted())
        .foregroundStyle(.secondary)
    }
    .font(.callout)
    .padding(12)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(.quaternary.opacity(0.45), in: .rect(cornerRadius: 8))
  }

  private func inferenceTiming(_ timing: Components.Schemas.InferenceTiming) -> some View {
    VStack(alignment: .leading, spacing: 7) {
      Divider()
      Text("Inference Timing").font(.headline).accessibilityAddTraits(.isHeader)
      Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
        metadataRow("Runtime to first token") { Text(TraceStep.elapsed(timing.promptMicros)) }
        metadataRow("Decode time") { Text(TraceStep.elapsed(timing.generationMicros)) }
        metadataRow("Prompt tokens processed") { Text(timing.processedPromptTokens.formatted()) }
        metadataRow("Prompt tokens cached") { Text(timing.cachedPromptTokens.formatted()) }
        metadataRow("Tokens generated") { Text(timing.generatedTokens.formatted()) }
      }
    }
  }

  private func fusionRanking(_ ranking: Components.Schemas.FusionRanking) -> some View {
    VStack(alignment: .leading, spacing: 7) {
      Divider()
      Text("Hybrid Fusion").font(.headline).accessibilityAddTraits(.isHeader)
      Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
        metadataRow("Lexical candidates") { Text(ranking.lexicalCandidates.formatted()) }
        metadataRow("Dense candidates") { Text(ranking.denseCandidates.formatted()) }
        metadataRow("Rank offset") { Text(ranking.rankOffset.formatted()) }
        metadataRow("Dense weight") { Text(ranking.denseWeight.formatted()) }
      }
      if ranking.candidates.isEmpty {
        Text("No candidates recorded.").foregroundStyle(.secondary)
      } else {
        ForEach(ranking.candidates, id: \.scoreRank) { candidate in
          fusionCandidate(candidate)
        }
      }
    }
    .accessibilityIdentifier("trace-fusion-ranking")
  }

  private func fusionCandidate(_ candidate: Components.Schemas.FusionCandidate) -> some View {
    VStack(alignment: .leading, spacing: 3) {
      HStack {
        if let finalRank = candidate.finalRank {
          Text("Fusion #\(finalRank)").fontWeight(.medium)
        } else {
          Text("Dropped at top-20 cutoff").fontWeight(.medium).foregroundStyle(.secondary)
        }
        if candidate.lexicalPromoted {
          Label("Lexical promotion", systemImage: "arrow.up.circle")
            .foregroundStyle(.secondary)
        }
        Spacer()
        Text("Score #\(candidate.scoreRank)")
      }
      HStack(spacing: 12) {
        if let lexicalRank = candidate.lexicalRank { Text("Lexical #\(lexicalRank)") }
        if let denseRank = candidate.denseRank { Text("Dense #\(denseRank)") }
        Text("Score \(candidate.score.formatted(.number.precision(.fractionLength(0...8))))")
      }
      .foregroundStyle(.secondary)
      .monospacedDigit()
    }
    .font(.callout)
    .padding(.vertical, 3)
    .accessibilityIdentifier("trace-fusion-candidate-\(candidate.scoreRank)")
  }

  private func metadataRow<Content: View>(
    _ title: LocalizedStringKey, @ViewBuilder value: () -> Content
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
