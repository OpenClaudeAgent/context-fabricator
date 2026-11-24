import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct GitHubSheetContent: View {
  let store: StoreOf<GitHubConnector>
  let traceInspector: StoreOf<TraceInspector>
  let answers: StoreOf<Answers>
  @State private var trace: CollectionTraceDestination?
  @State private var picker = GitHubPickerInput()

  var body: some View {
    content.sheet(item: $trace) { destination in
      TraceInspectorView(
        store: traceInspector, traceID: destination.traceID, initialSpanID: destination.spanID,
        answers: answers)
    }
  }

  @ViewBuilder private var content: some View {
    switch store.panel {
    case .account, .repositoryAccount:
      if let snapshot = store.snapshot {
        GitHubAccountSheet(store: store, snapshot: snapshot)
      }
    case .picker:
      GitHubPickerSheet(store: store, input: $picker)
    case .addOptions, .editOptions:
      if let draft = store.draft {
        GitHubOptionsSheet(store: store, draft: draft, showTrace: showTrace)
      }
    case .graph:
      GitHubGraphSheet(store: store)
    case .actions:
      GitHubActionsSheet(store: store)
    case .traffic:
      if let traffic = store.snapshot?.traffic {
        GitHubTrafficSheet(store: store, traffic: traffic)
      }
    case nil:
      EmptyView()
    }
  }

  private func showTrace(_ trace: Components.Schemas.GitHubCollectionTrace) {
    self.trace = CollectionTraceDestination(traceID: trace.traceId, spanID: trace.spanId)
  }
}

private struct CollectionTraceDestination: Identifiable {
  let traceID: String
  let spanID: String
  var id: String { "\(traceID):\(spanID)" }
}

struct GitHubCollectionTraceLinks: View {
  @Environment(\.locale) private var locale
  let activity: Components.Schemas.GitHubCollectionActivity?
  let feature: String
  let showTrace: (Components.Schemas.GitHubCollectionTrace) -> Void

  var body: some View {
    if let activity {
      let latest = activity.latestAttempt
      let published = activity.publishedSnapshot
      if let latest, let published,
        latest.traceId == published.traceId && latest.spanId == published.spanId
      {
        traceButton(locale.localized("View collection trace"), id: "collection", trace: latest)
      } else {
        if let latest {
          traceButton(locale.localized("View latest attempt trace"), id: "latest", trace: latest)
        }
        if let published {
          traceButton(
            locale.localized("View published snapshot trace"), id: "published", trace: published)
        }
      }
    }
  }

  private func traceButton(
    _ title: String, id: String, trace: Components.Schemas.GitHubCollectionTrace
  ) -> some View {
    Button(title, systemImage: "waveform.path") { showTrace(trace) }
      .controlSize(.small)
      .help(feature == "history" ? locale.localized("\(title) for code history") : title)
      .accessibilityIdentifier("github-\(feature)-trace-\(id)")
  }
}

struct GitHubSheetWidth {
  static let standard = Self(minimum: 450, ideal: 540, maximum: 640)
  static let wide = Self(minimum: 760, ideal: 920, maximum: 1060)

  let minimum: CGFloat
  let ideal: CGFloat
  let maximum: CGFloat
}

struct GitHubSheet<Content: View, Buttons: View>: View {
  let title: LocalizedStringKey
  var subtitle: String?
  let message: String?
  let busy: Bool
  var width: GitHubSheetWidth = .standard
  var prefersTallContent = false
  @ViewBuilder let content: Content
  @ViewBuilder let buttons: Buttons

  private var tallContentHeight: CGFloat? {
    guard prefersTallContent else { return nil }
    let screenHeight = NSApp.keyWindow?.screen?.visibleFrame.height
      ?? NSScreen.main?.visibleFrame.height ?? 820
    // Leave room for the parent toolbar, sheet header, feedback, and buttons.
    return min(550, max(340, screenHeight - 230))
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: 12) {
        GitHubIcon(size: 36)
        VStack(alignment: .leading, spacing: 3) {
          Text(title).font(.title2.bold()).accessibilityAddTraits(.isHeader)
          if let subtitle {
            Text(verbatim: subtitle).font(.subheadline).foregroundStyle(.secondary)
              .lineLimit(1).truncationMode(.middle).help(subtitle)
          }
        }
        Spacer()
        if busy { ProgressView().controlSize(.small).accessibilityLabel("Working") }
      }
      .padding(.horizontal, 22)
      .padding(.vertical, 18)
      Divider()
      content.frame(height: tallContentHeight).disabled(busy)
      Divider()
      VStack(alignment: .leading, spacing: 10) {
        if let message { GitHubFeedback(message: message) }
        HStack { buttons }
      }
      .padding(.horizontal, 22)
      .padding(.vertical, 14)
    }
    .frame(minWidth: width.minimum, idealWidth: width.ideal, maxWidth: width.maximum)
  }
}
