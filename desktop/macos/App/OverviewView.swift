import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct OverviewView: View {
  let store: StoreOf<Dashboard>

  var body: some View {
    WorkspacePage {
      HStack(alignment: .firstTextBaseline, spacing: 16) {
        WorkspaceHeading(title: "Overview")
        Button("Ask a question", systemImage: "bubble.left") {
          store.send(.destinationChanged(.answers))
        }
        .buttonStyle(.borderedProminent)
        .disabled(store.connection != .ready)
      }

      switch store.connection {
      case .connecting:
        ProgressView("Connecting to the local backend")
      case .ready:
        if let status = store.snapshot?.status {
          corpus(status)
          serviceDetails(status)
        }
      case .unavailable(let message):
        WorkspaceGroup(title: "Local service") {
          VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
              Image(systemName: "exclamationmark.circle")
                .foregroundStyle(.orange)
                .accessibilityHidden(true)
              Text("Backend unavailable").accessibilityIdentifier("backend-unavailable")
            }
            .font(.headline)
            Text(message).foregroundStyle(.secondary)
            restart
          }
        }
      }
    }
    .labeledContentStyle(WorkspaceRowStyle())
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
          Label(status.status.rawValue.capitalized, systemImage: "checkmark.circle.fill")
            .foregroundStyle(.green)
            .accessibilityIdentifier("backend-ready")
        }
        Divider()
        if let retrieval = status.retrieval {
          LabeledContent("Embedding index", value: "\(retrieval.indexed) / \(retrieval.total)")
          Divider()
          LabeledContent("Embedding state", value: retrieval.state.rawValue.capitalized)
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
