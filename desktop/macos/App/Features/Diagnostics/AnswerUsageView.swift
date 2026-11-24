import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct AnswerUsageView: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<AnswerUsage>
  let ready: Bool
  let showTrace: (String, String?) -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack {
        VStack(alignment: .leading, spacing: 3) {
          Text("Retained Answer Requests").font(.headline)
          Text("Backend latency for published answers · up to 500 retained requests")
            .font(.caption).foregroundStyle(.secondary)
          if let snapshot = store.snapshot, let first = snapshot.firstAt, let last = snapshot.lastAt
          {
            Text(first == last ? date(first) : "\(date(first)) – \(date(last))")
              .font(.caption).foregroundStyle(.secondary)
          }
        }
        Spacer()
        if store.isLoading && store.snapshot?.groups.isEmpty == false {
          ProgressView("Loading answer usage…").labelsHidden().controlSize(.small)
        }
      }
      if let feedback = store.feedback {
        Label(
          store.snapshot == nil ? feedback : "Refresh failed. Showing the last successful reading.",
          systemImage: "exclamationmark.circle"
        )
        .foregroundStyle(.orange)
        .help(feedback)
      }
      if let snapshot = store.snapshot, !snapshot.groups.isEmpty {
        ScrollView {
          LazyVStack(spacing: 10) {
            ForEach(Array(snapshot.groups.enumerated()), id: \.offset) { index, group in
              usageRow(group, index: index)
            }
          }
        }
      } else {
        Group {
          if store.isLoading {
            LoadingStateView("Loading answer usage…")
          } else {
            ContentUnavailableView(
              ready && store.feedback == nil ? "No Answer Usage Yet" : "Answer Usage Unavailable",
              systemImage: "chart.bar"
            )
            .fixedSize(horizontal: false, vertical: true)
          }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
      }
    }.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
  }

  private func usageRow(_ group: Components.Schemas.AnswerUsageGroup, index: Int) -> some View {
    let profile = group.modelProfile ?? locale.localized("Model unspecified")
    return GroupBox {
      VStack(alignment: .leading, spacing: 10) {
        HStack(alignment: .top, spacing: 12) {
          VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
              Image(systemName: clientSymbol(group.client))
                .foregroundStyle(.secondary).accessibilityHidden(true)
              Text(clientName(group.client)).font(.headline)
                .accessibilityIdentifier("answer-usage-client-\(index)")
            }
            Text(profile).font(.caption.monospaced()).foregroundStyle(.secondary)
              .lineLimit(1).truncationMode(.middle).textSelection(.enabled).help(profile)
              .accessibilityIdentifier("answer-usage-model-\(index)")
          }
          Spacer(minLength: 0)
          if let trace = group.latestTraceId {
            Button("Latest Trace", systemImage: "waveform.path") { showTrace(trace, nil) }
              .fixedSize()
              .accessibilityIdentifier("answer-usage-trace-\(index)")
          }
        }
        Divider()
        HStack(spacing: 8) {
          metric(
            "Published", value: group.published.formatted(), id: "answer-usage-published-\(index)",
            tint: group.published > 0 ? .green : .primary)
          metric(
            "Abstained", value: group.abstained.formatted(),
            tint: group.abstained > 0 ? .orange : .primary)
          metric("Cancelled", value: group.cancelled.formatted())
          metric(
            "Failed", value: group.failed.formatted(), tint: group.failed > 0 ? .red : .primary)
          metric(
            "Unknown", value: group.unknown.formatted(),
            tint: group.unknown > 0 ? .orange : .primary)
        }
        HStack(spacing: 16) {
          LabeledContent("Median", value: duration(group.p50Micros))
          LabeledContent("P95", value: duration(group.p95Micros))
        }
        .font(.caption).monospacedDigit()
      }
      .padding(6)
    }
  }

  private func metric(
    _ label: String, value: String, id: String? = nil, tint: Color = .primary
  ) -> some View {
    VStack(alignment: .leading, spacing: 3) {
      Text(label).font(.caption).foregroundStyle(.secondary)
      Text(value).font(.title3.monospacedDigit().weight(.semibold)).foregroundStyle(tint)
        .accessibilityIdentifier(id ?? label)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  private func clientName(_ client: Components.Schemas.ClientKind) -> String {
    switch client {
    case .desktop: locale.localized("macOS")
    case .cli: locale.localized("CLI")
    case .other: locale.localized("Other / untagged")
    }
  }

  private func clientSymbol(_ client: Components.Schemas.ClientKind) -> String {
    switch client {
    case .desktop: "macwindow"
    case .cli: "terminal"
    case .other: "ellipsis.circle"
    }
  }

  private func duration(_ micros: Int64?) -> String {
    micros.map(TraceStep.elapsed) ?? "—"
  }

  private func date(_ seconds: Int64) -> String {
    Date(timeIntervalSince1970: Double(seconds)).formatted(date: .abbreviated, time: .shortened)
  }
}
