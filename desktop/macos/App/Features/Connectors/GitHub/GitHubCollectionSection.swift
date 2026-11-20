import LocalAPI
import SwiftUI

struct GitHubCollectionSection: View {
  let status: Components.Schemas.GitHubCodeCollection

  var body: some View {
    Section("Collection Timing") {
      if let published = status.publishedAtMs {
        timestamp("Published", milliseconds: published)
      }
      if let checked = status.lastCheckedAtMs {
        timestamp("Last checked", milliseconds: checked)
      }
      if let next = status.nextAtMs {
        timestamp("Next check", milliseconds: next)
      }
      if let error = status.error {
        Text(error).foregroundStyle(.secondary)
      }
    }
  }

  private func timestamp(_ title: LocalizedStringKey, milliseconds: Int64) -> some View {
    LabeledContent(title) {
      Text(
        Date(timeIntervalSince1970: Double(milliseconds) / 1_000),
        format: .dateTime.month().day().hour().minute()
      )
    }
  }
}

struct GitHubCollectionSummary: View {
  let status: Components.Schemas.GitHubCodeCollection

  var body: some View {
    LabeledContent("Current code") {
      HStack(spacing: 10) {
        if let commit = status.commit {
          Text(commit.prefix(10))
            .font(.body.monospaced())
            .help(commit)
            .accessibilityIdentifier("github-indexed-commit")
        }
        GitHubCollectionLabel(phase: status.phase)
          .accessibilityIdentifier("github-collection-status")
      }
    }
  }
}

struct GitHubCollectionLabel: View {
  let phase: Components.Schemas.GitHubCodeCollection.phasePayload

  var body: some View {
    switch phase {
    case .disabled: Text("Collection off")
    case .credentials: Text("Read access required")
    case .queued: Label("Queued", systemImage: "clock")
    case .running: Label("Refreshing", systemImage: "arrow.trianglehead.clockwise")
    case .succeeded: Label("Up to date", systemImage: "checkmark.circle")
    case .retry: Label("Retry scheduled", systemImage: "clock.arrow.circlepath")
    case .throttled: Label("Waiting for GitHub", systemImage: "hourglass")
    case .failed: Label("Refresh failed", systemImage: "exclamationmark.circle")
    }
  }
}
