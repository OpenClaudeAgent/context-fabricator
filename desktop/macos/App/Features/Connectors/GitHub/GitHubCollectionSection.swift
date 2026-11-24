import LocalAPI
import SwiftUI

struct GitHubCollectionSection: View {
  let status: Components.Schemas.GitHubCodeCollection
  let title: LocalizedStringKey
  let canRefresh: Bool
  let refresh: () -> Void

  var body: some View {
    Section {
      LabeledContent(title) {
        Button("Refresh Now", action: refresh)
          .controlSize(.small)
          .disabled(!canRefresh)
          .accessibilityIdentifier("github-refresh-code")
      }
      LabeledContent("Status") {
        HStack(spacing: 6) {
          if status.phase == .running {
            ProgressView().controlSize(.small).accessibilityHidden(true)
            Text(
              status.pending?.phase == .manifestComplete ? "Preparing index" : "Syncing files"
            )
            .accessibilityIdentifier("github-collection-status")
          } else {
            GitHubCollectionLabel(phase: status.phase)
              .accessibilityIdentifier("github-collection-status")
          }
        }
      }
      if let pending = status.pending {
        LabeledContent("Files prepared") {
          Text(pending.stagedFiles, format: .number)
            .accessibilityIdentifier("github-staged-source-files")
        }
        .help("Includes unchanged files reused from the previous sync.")
        LabeledContent("Commit being synced") {
          Text(pending.commit.prefix(10))
            .font(.body.monospaced())
            .help(pending.commit)
            .accessibilityIdentifier("github-pending-commit")
        }
      }
      if let commit = status.commit {
        LabeledContent("Local commit") {
          Text(commit.prefix(10)).font(.body.monospaced()).help(commit)
            .accessibilityIdentifier("github-indexed-commit")
        }
      }
      if let publishedFiles = status.publishedFiles {
        LabeledContent("Files in local copy") {
          Text(publishedFiles, format: .number)
            .accessibilityIdentifier("github-published-source-files")
        }
      }
      if let checked = status.lastCheckedAtMs, status.phase != .running {
        timestamp("Last checked", milliseconds: checked)
      }
      if let next = status.nextAtMs,
        status.phase == .succeeded || status.phase == .throttled
          || status.phase == .retry
      {
        timestamp("Next check", milliseconds: next)
      }
    } header: {
      Text("Branch Sync")
    } footer: {
      if let error = status.error, status.phase == .failed || status.phase == .retry {
        Label(error, systemImage: "exclamationmark.circle")
      } else if status.pending != nil, status.commit == nil {
        Text("The first sync must finish before this branch is available in the app.")
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

struct GitHubCollectionListStatus: View {
  let status: Components.Schemas.GitHubCodeCollection

  var body: some View {
    Group {
      GitHubCollectionLabel(phase: status.phase)
      if let pending = status.pending {
        Text("Staged files · \(pending.stagedFiles.formatted())")
      }
    }
    .font(.caption)
    .foregroundStyle(.secondary)
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
