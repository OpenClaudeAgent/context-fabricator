import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct DashboardView: View {
  private enum PresentedSheet: Identifiable {
    case modelLibrary
    case answerSetup
    case answerPipeline
    case trace(id: String, spanID: String?)

    var id: String {
      switch self {
      case .modelLibrary: "model-library"
      case .answerSetup: "answer-setup"
      case .answerPipeline: "answer-pipeline"
      case .trace(let id, _): id
      }
    }
  }

  let store: StoreOf<Dashboard>
  let inferenceDirectory: URL
  let supervisor: BackendSupervisor
  @Binding var appLanguage: AppLanguage
  @State private var presentedSheet: PresentedSheet?
  @State private var openDownloadLibrary = false
  @State private var pipelineSources = AnswerPipelineSources()

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
              refreshButton
            }
          }
        }
    }
    .navigationSplitViewStyle(.balanced)
    .sheet(item: $presentedSheet, onDismiss: openDownloadsAfterSetup, content: sheet)
    .task { await store.send(.refresh).finish() }
    .task { await store.send(.observeBackend).finish() }
    .onDisappear {
      store.send(.traceInspector(.close))
      store.send(.answerHistory(.discardCachedEntries))
    }
  }

  private var refreshButton: some View {
    let usage = store.destination == .diagnostics && store.diagnosticTab == .answerUsage
    return Button("Refresh", systemImage: "arrow.clockwise") {
      if usage { store.send(.answerUsage(.refresh)) } else { store.send(.refresh) }
    }
    .keyboardShortcut("r")
    .disabled(
      usage
        ? store.connection != .ready || store.answerUsage.isLoading
        : store.connection == .connecting || store.telemetryBusy
    )
    .help(usage ? "Refresh answer usage" : "Refresh backend status")
    .accessibilityIdentifier(usage ? "answer-usage-refresh" : "dashboard-refresh")
  }

  private func destination(_ value: Dashboard.Destination, icon: String) -> some View {
    Label(value.localizedTitle, systemImage: icon)
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
        history: store.scope(state: \.answerHistory, action: \.answerHistory),
        connection: store.connection, restart: { store.send(.restart) },
        showModelLibrary: showModelLibrary, showSetup: { presentedSheet = .answerSetup },
        showPipeline: { presentedSheet = .answerPipeline },
        showHistorySettings: { store.send(.destinationChanged(.settings)) },
        showTrace: { showTrace($0, spanID: nil) })
    case .connectors:
      GitHubView(
        store: store.scope(state: \.github, action: \.github),
        traceInspector: store.scope(state: \.traceInspector, action: \.traceInspector),
        answers: store.scope(state: \.answers, action: \.answers))
    case .diagnostics:
      DiagnosticsView(store: store, showTrace: { showTrace($0, spanID: $1) })
    case .settings:
      PreferencesView(
        store: store, showModelLibrary: showModelLibrary, appLanguage: $appLanguage
      )
    }
  }

  @ViewBuilder private func sheet(_ destination: PresentedSheet) -> some View {
    switch destination {
    case .modelLibrary:
      ModelLibraryView(
        store: store.scope(state: \.modelLibrary, action: \.modelLibrary),
        inferenceDirectory: inferenceDirectory,
        connection: store.connection,
        restartBlocked: store.answers.answerBusy || store.answers.modelSwitching
      ) { store.send(.restart) }
      .onDisappear {
        if store.connection == .ready {
          store.send(.answers(.loadModels))
          if store.destination == .settings { store.send(.settingsForm(.loadEmbeddingModels)) }
        }
      }
    case .answerPipeline:
      AnswerPipelineView(
        supervisor: supervisor, initialQuestion: store.answers.question,
        initialRepository: store.answers.selectedCodeRepository, sources: pipelineSources)
    case .answerSetup:
      DirectInferenceSetupView(
        store: store.scope(state: \.modelLibrary, action: \.modelLibrary)
      ) {
        openDownloadLibrary = true
        presentedSheet = nil
      }
      .onDisappear {
        if store.connection == .ready {
          store.send(.answers(.loadModels))
          if store.destination == .settings { store.send(.settingsForm(.loadEmbeddingModels)) }
        }
      }
    case .trace(let id, let spanID):
      TraceInspectorView(
        store: store.scope(state: \.traceInspector, action: \.traceInspector), traceID: id,
        initialSpanID: spanID, answers: store.scope(state: \.answers, action: \.answers))
    }
  }

  private func showModelLibrary() {
    presentedSheet = .modelLibrary
  }

  private func openDownloadsAfterSetup() {
    guard openDownloadLibrary else { return }
    openDownloadLibrary = false
    showModelLibrary()
  }

  private func showTrace(_ id: String, spanID: String?) {
    presentedSheet = .trace(id: id, spanID: spanID)
  }
}

struct WorkspaceHeading: View {
  let title: LocalizedStringKey

  var body: some View {
    Text(title)
      .font(.title2.bold())
      .accessibilityAddTraits(.isHeader)
      .frame(maxWidth: .infinity, alignment: .leading)
  }
}

struct WorkspacePage<Content: View>: View {
  var fillsViewport = false
  @ViewBuilder let content: Content

  var body: some View {
    if fillsViewport {
      GeometryReader { viewport in page(minimumHeight: viewport.size.height) }
    } else {
      page(minimumHeight: nil)
    }
  }

  private func page(minimumHeight: CGFloat?) -> some View {
    ScrollView {
      WorkspaceColumn(minimumHeight: minimumHeight) {
        VStack(alignment: .leading, spacing: 20) { content }
          .padding(.bottom, 24)
      }
    }
  }
}

struct WorkspaceColumn<Content: View>: View {
  var minimumHeight: CGFloat?
  @ViewBuilder let content: Content

  var body: some View {
    content
      .frame(maxWidth: 760, alignment: .leading)
      .padding(.horizontal, 24)
      .padding(.top, 20)
      .frame(minHeight: minimumHeight, alignment: .top)
      .frame(maxWidth: .infinity)
  }
}

struct LoadingStateView: View {
  let title: LocalizedStringKey

  init(_ title: LocalizedStringKey) {
    self.title = title
  }

  var body: some View {
    VStack(spacing: 8) {
      ProgressView().accessibilityHidden(true)
      Text(title).font(.callout).foregroundStyle(.secondary)
    }
    .multilineTextAlignment(.center)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
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
