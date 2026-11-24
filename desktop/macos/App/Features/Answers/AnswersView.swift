import ComposableArchitecture
import DesktopFeature
import SwiftUI

struct AnswersView: View {
  @Environment(\.locale) private var locale
  private enum HistoryDestination {
    case settings
    case trace(String)
  }

  let store: StoreOf<Answers>
  let history: StoreOf<AnswerHistory>
  @State private var showsHistory = false
  @State private var pendingHistoryDestination: HistoryDestination?
  let connection: Dashboard.Connection
  let restart: () -> Void
  let showModelLibrary: () -> Void
  let showSetup: () -> Void
  let showPipeline: () -> Void
  let showHistorySettings: () -> Void
  let showTrace: (String) -> Void

  var body: some View {
    WorkspacePage(fillsViewport: true) {
      HStack {
        WorkspaceHeading(title: "Answers")
        Spacer()
        Button("Inspect Pipeline…", systemImage: "scope", action: showPipeline)
          .disabled(connection != .ready || store.answerBusy || store.modelSwitching)
          .accessibilityIdentifier("open-answer-pipeline")
        Button("History…", systemImage: "clock.arrow.circlepath") { showsHistory = true }
          .disabled(connection != .ready)
          .accessibilityIdentifier("answer-history")
      }
      if connection == .ready, store.models != nil, store.activeModel == nil {
        HStack {
          Label("Set up a local model to start asking questions.", systemImage: "sparkles")
          Spacer()
          Button("Choose a Model…", action: showSetup)
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("set-up-answers")
        }
        .padding(12)
        .background(.quaternary, in: .rect(cornerRadius: 8))
      }
      AnswerComposer(
        store: store, connection: connection, restart: restart, showModelLibrary: showModelLibrary)
      if let message = store.answerFeedback {
        HStack(spacing: 8) {
          Image(systemName: "info.circle").accessibilityHidden(true)
          Text(locale.localizedKey(message)).accessibilityIdentifier("answer-feedback")
          Spacer()
          if let id = store.answerTraceID { traceButton(id) }
        }
        .foregroundStyle(.secondary)
      }
      if let answer = store.answer {
        AnswerResult(
          answer: answer, viewTrace: store.answerTraceID.map { id in { showTrace(id) } })
      } else if !store.answerBusy && store.answerFeedback == nil
        && store.models?.activeProfile != nil
      {
        ContentUnavailableView(
          "Ask about your corpus",
          systemImage: "text.bubble",
          description: Text("Answers appear here with citations from your local sources.")
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
      }
    }
    .sheet(
      isPresented: $showsHistory,
      onDismiss: {
        let destination = pendingHistoryDestination
        pendingHistoryDestination = nil
        switch destination {
        case .settings: showHistorySettings()
        case .trace(let id): showTrace(id)
        case nil: break
        }
      }
    ) {
      AnswerHistoryView(
        store: history, answers: store,
        showSettings: {
          pendingHistoryDestination = .settings
          showsHistory = false
        }
      ) { id in
        pendingHistoryDestination = .trace(id)
        showsHistory = false
      }
    }
    .task(id: connection == .ready) {
      if connection == .ready {
        await store.send(.loadModels).finish()
      }
    }
  }

  private func traceButton(_ id: String) -> some View {
    Button("View Trace", systemImage: "waveform.path") { showTrace(id) }
      .accessibilityIdentifier("view-answer-trace")
  }
}
