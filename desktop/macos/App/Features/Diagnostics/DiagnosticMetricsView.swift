import Foundation
import LocalAPI
import SwiftUI

struct DiagnosticMetricsView: View {
  @Environment(\.locale) private var locale
  let snapshot: Components.Schemas.TelemetrySnapshot?
  let query: String

  var body: some View {
    let filter = query.trimmingCharacters(in: .whitespacesAndNewlines)
    let counters = (snapshot?.metrics ?? []).map(Counter.init).filter {
      $0.matches(filter, in: locale)
    }
    VStack(alignment: .leading, spacing: 12) {
      VStack(alignment: .leading, spacing: 4) {
        Text("Latest Operation Counts").font(.headline).accessibilityAddTraits(.isHeader)
        Text(
          "Counts reset when the backend restarts. The latest retained reading can come from an earlier session."
        )
        .font(.caption).foregroundStyle(.secondary)
      }
      if counters.isEmpty {
        emptyState(filter: filter)
          .fixedSize(horizontal: false, vertical: true)
          .frame(maxWidth: .infinity, maxHeight: .infinity)
      } else {
        Table(counters) {
          TableColumn("Operation") { counter in
            Text(counter.title(in: locale))
              .help(counter.sample.operation ?? counter.sample.name)
              .accessibilityIdentifier("telemetry-metric")
          }
          TableColumn("Completed operations") { counter in
            Text(counter.sample.value?.formatted() ?? "—")
              .monospacedDigit()
              .frame(maxWidth: .infinity, alignment: .trailing)
          }
          .width(min: 145, ideal: 160)
          TableColumn("Last observed") { counter in
            Text(counter.observedAt, format: .dateTime.month().day().hour().minute().second())
              .foregroundStyle(.secondary)
              .monospacedDigit()
          }
          .width(min: 150, ideal: 180)
        }
        .alternatingRowBackgrounds(.disabled)
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
  }

  @ViewBuilder private func emptyState(filter: String) -> some View {
    if snapshot == nil {
      ContentUnavailableView("Metrics Unavailable", systemImage: "chart.bar")
    } else if !filter.isEmpty {
      ContentUnavailableView.search(text: filter)
    } else {
      ContentUnavailableView("No Metrics Yet", systemImage: "chart.bar")
    }
  }
}

private struct Counter: Identifiable {
  let sample: Components.Schemas.SignalSample

  var id: Int64 { sample.id }
  func title(in locale: Locale) -> String {
    sample.displayName.map { locale.localizedKey($0) }
      ?? sample.name.replacingOccurrences(of: "_", with: " ").capitalized
  }
  var observedAt: Date { Date(timeIntervalSince1970: TimeInterval(sample.timestamp)) }

  func matches(_ filter: String, in locale: Locale) -> Bool {
    filter.isEmpty
      || [sample.name, title(in: locale), sample.displayName ?? "", sample.operation ?? ""].contains
      {
        $0.localizedCaseInsensitiveContains(filter)
      }
  }
}
