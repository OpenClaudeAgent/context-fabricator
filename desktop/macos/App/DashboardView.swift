import ComposableArchitecture
import DesktopFeature
import Foundation
import SwiftUI

struct DashboardView: View {
  private struct PresentedTrace: Identifiable {
    let id: String
    let spanID: String?
  }

  let store: StoreOf<Dashboard>
  let inferenceDirectory: URL
  @State private var presentedTrace: PresentedTrace?

  var body: some View {
    NavigationSplitView {
      List(
        selection: Binding<Dashboard.Destination?>(
          get: { store.destination },
          set: { if let destination = $0 { store.send(.destinationChanged(destination)) } })
      ) {
        Section("Workspace") {
          destination(.overview, icon: "square.grid.2x2")
          destination(.answers, icon: "bubble.left.and.text.bubble.right")
          destination(.connectors, icon: "point.3.connected.trianglepath.dotted")
        }
        Section("System") {
          destination(.diagnostics, icon: "waveform.path")
          destination(.settings, icon: "gearshape")
        }
      }
      .listStyle(.sidebar)
      .navigationSplitViewColumnWidth(min: 175, ideal: 212, max: 256)
    } detail: {
      screen
        .frame(minWidth: 480, maxWidth: .infinity, minHeight: 440, maxHeight: .infinity)
        .toolbar {
          if store.destination == .overview || store.destination == .diagnostics {
            ToolbarItem(placement: .primaryAction) {
              Button("Refresh", systemImage: "arrow.clockwise") { store.send(.refresh) }
                .keyboardShortcut("r")
                .disabled(store.connection == .connecting || store.telemetryBusy)
                .help("Refresh backend status")
            }
          }
        }
    }
    .navigationSplitViewStyle(.balanced)
    .sheet(item: $presentedTrace) { trace in
      TraceInspectorView(
        store: store.scope(state: \.traceInspector, action: \.traceInspector), traceID: trace.id,
        initialSpanID: trace.spanID)
    }
    .task { await store.send(.refresh).finish() }
    .task { await store.send(.observeBackend).finish() }
  }

  private func destination(_ value: Dashboard.Destination, icon: String) -> some View {
    Label(value.rawValue, systemImage: icon)
      .tag(value)
      .accessibilityIdentifier("navigation-\(value.rawValue.lowercased())")
  }

  @ViewBuilder private var screen: some View {
    switch store.destination {
    case .overview:
      OverviewView(store: store)
    case .answers:
      AnswersView(
        store: store.scope(state: \.answers, action: \.answers),
        modelLibrary: store.scope(state: \.modelLibrary, action: \.modelLibrary),
        connection: store.connection, inferenceDirectory: inferenceDirectory,
        restart: { store.send(.restart) }, showTrace: { showTrace($0, spanID: nil) })
    case .connectors:
      GitHubView(store: store.scope(state: \.github, action: \.github))
    case .diagnostics:
      DiagnosticsView(store: store, showTrace: showTrace)
    case .settings:
      PreferencesView(store: store)
    }
  }

  private func showTrace(_ id: String, spanID: String?) {
    presentedTrace = .init(id: id, spanID: spanID)
  }
}

struct WorkspaceHeading: View {
  let title: String

  var body: some View {
    Text(title)
      .font(.title2.bold())
      .accessibilityAddTraits(.isHeader)
      .frame(maxWidth: .infinity, alignment: .leading)
  }
}

struct WorkspacePage<Content: View>: View {
  @ViewBuilder let content: Content

  var body: some View {
    ScrollView {
      WorkspaceColumn {
        VStack(alignment: .leading, spacing: 20) { content }
          .padding(.bottom, 24)
      }
    }
  }
}

struct WorkspaceColumn<Content: View>: View {
  @ViewBuilder let content: Content

  var body: some View {
    content
      .frame(maxWidth: 760, alignment: .leading)
      .padding(.horizontal, 24)
      .padding(.top, 20)
      .frame(maxWidth: .infinity)
  }
}

struct WorkspaceGroup<Content: View>: View {
  let title: LocalizedStringKey
  @ViewBuilder let content: Content

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      Text(title).font(.headline).accessibilityAddTraits(.isHeader)
      GroupBox {
        content
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(.horizontal, 8)
          .padding(.vertical, 6)
      }
    }
  }
}

struct WorkspaceRowStyle: LabeledContentStyle {
  func makeBody(configuration: Configuration) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 12) {
      configuration.label
      Spacer(minLength: 0)
      configuration.content.multilineTextAlignment(.trailing)
    }
  }
}
