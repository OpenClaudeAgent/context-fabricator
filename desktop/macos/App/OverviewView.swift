import AppKit
import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct OverviewView: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<Dashboard>

  var body: some View {
    Group {
      if store.connection == .connecting {
        VStack(spacing: 20) {
          WorkspaceColumn { header }
          LoadingStateView("Connecting to the local backend")
        }
      } else {
        overview
      }
    }
    .labeledContentStyle(WorkspaceRowStyle())
    .task(id: store.connection == .ready) {
      if store.connection == .ready { await store.send(.observeOverview).finish() }
    }
  }

  private var header: some View {
    HStack(alignment: .firstTextBaseline, spacing: 16) {
      WorkspaceHeading(title: "Overview")
      Button("Ask a question", systemImage: "bubble.left") {
        store.send(.destinationChanged(.answers))
      }
      .buttonStyle(.borderedProminent)
      .disabled(store.connection != .ready)
    }
  }

  private var overview: some View {
    WorkspacePage {
      header
      switch store.connection {
      case .connecting: EmptyView()
      case .ready:
        if let status = store.snapshot?.status {
          corpus(status)
          serviceDetails(status)
        }
      case .dataDirectoryInUse, .unavailable:
        WorkspaceGroup(title: "Local service") {
          VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
              Image(systemName: "exclamationmark.circle")
                .foregroundStyle(.orange)
                .accessibilityHidden(true)
              Text(
                store.connection == .dataDirectoryInUse
                  ? "Context Fabricator is already open" : "Backend unavailable"
              ).accessibilityIdentifier("backend-unavailable")
            }
            .font(.headline)
            if let message = store.connection.failureMessage {
              Text(locale.localizedKey(message)).foregroundStyle(.secondary)
            }
            HStack {
              if store.connection == .dataDirectoryInUse, let app = otherInstance {
                Button("Show Running App", systemImage: "arrow.up.forward.app") {
                  _ = app.activate()
                }
                .buttonStyle(.borderedProminent)
              }
              restart
            }
          }
        }
      }
    }
  }

  private var otherInstance: NSRunningApplication? {
    guard let id = Bundle.main.bundleIdentifier else { return nil }
    return NSRunningApplication.runningApplications(withBundleIdentifier: id).first {
      !$0.isTerminated && $0.processIdentifier != NSRunningApplication.current.processIdentifier
    }
  }

  private func corpus(_ status: Components.Schemas.Status) -> some View {
    WorkspaceGroup(title: "Corpus") {
      VStack(spacing: 8) {
        corpusRow(
          "Searchable documents", value: status.indexedDocuments, icon: "doc.text.magnifyingglass"
        )
        .accessibilityIdentifier("searchable-documents")
        if let collected = status.collectedDocuments {
          Divider()
          corpusRow("Collected documents", value: collected, icon: "tray.full")
            .accessibilityIdentifier("collected-documents")
        }
        Divider()
        corpusRow("Connected sources", value: status.connectedSources, icon: "link")
      }
    }
  }

  private func corpusRow(_ title: LocalizedStringKey, value: Int, icon: String) -> some View {
    LabeledContent {
      Text(value.formatted()).monospacedDigit()
    } label: {
      Label {
        Text(title)
      } icon: {
        Image(systemName: icon).frame(width: 16)
      }
    }
    .accessibilityElement(children: .combine)
  }

  private func serviceDetails(_ status: Components.Schemas.Status) -> some View {
    ViewThatFits(in: .horizontal) {
      HStack(alignment: .top, spacing: 16) {
        localService(status).frame(minWidth: 280)
        outboundTraffic(status).frame(minWidth: 224)
      }
      VStack(alignment: .leading, spacing: 20) {
        localService(status)
        outboundTraffic(status)
      }
    }
  }

  private func localService(_ status: Components.Schemas.Status) -> some View {
    WorkspaceGroup(title: "Local service") {
      VStack(spacing: 8) {
        LabeledContent("Backend") {
          Label("Ready", systemImage: "checkmark.circle.fill")
            .foregroundStyle(.green)
            .accessibilityIdentifier("backend-ready")
        }
        Divider()
        if let retrieval = status.retrieval {
          LabeledContent(
            "Embedding index",
            value: retrieval.state == .notConfigured
              ? "—" : locale.localized("\(retrieval.indexed) / \(retrieval.total) passages")
          )
          .accessibilityIdentifier("embedding-index-progress")
          Divider()
          LabeledContent("Embedding state", value: embeddingState(retrieval.state))
            .accessibilityIdentifier("embedding-index-state")
        } else {
          LabeledContent("Local inference", value: "Awaiting configuration")
        }
        Divider()
        HStack {
          Spacer()
          restart
        }
      }
    }
  }

  private func embeddingState(_ state: Components.Schemas.RetrievalProgress.statePayload) -> String
  {
    switch state {
    case .ready: locale.localized("Ready")
    case .incomplete: locale.localized("Indexing")
    case .notConfigured: locale.localized("Not configured")
    case .disabled: locale.localized("Off")
    }
  }

  private func outboundTraffic(_ status: Components.Schemas.Status) -> some View {
    WorkspaceGroup(title: "Outbound traffic") {
      VStack(spacing: 8) {
        LabeledContent("Outbound attempts", value: status.outboundAttempts.formatted())
        if let pending = status.outboundPending {
          Divider()
          LabeledContent("Pending outcomes", value: pending.formatted())
        }
        if let uncertain = status.outboundUncertain {
          Divider()
          LabeledContent("Uncertain outcomes", value: uncertain.formatted())
        }
      }
      .monospacedDigit()
    }
  }

  private var restart: some View {
    Button("Restart backend", systemImage: "arrow.clockwise") { store.send(.restart) }
  }
}
