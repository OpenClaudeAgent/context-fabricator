import ComposableArchitecture
import DesktopFeature
import Foundation
import SwiftUI

struct AnswersView: View {
  let store: StoreOf<Answers>
  let modelLibrary: StoreOf<ModelLibrary>
  let connection: Dashboard.Connection
  let inferenceDirectory: URL
  let restart: () -> Void
  let showTrace: (String) -> Void
  @State private var showingModelLibrary = false

  var body: some View {
    WorkspacePage {
      WorkspaceHeading(title: "Answers")
      AnswerComposer(store: store, connection: connection, restart: restart) {
        showingModelLibrary = true
      }
      if let message = store.answerFeedback {
        HStack(spacing: 8) {
          Image(systemName: "info.circle").accessibilityHidden(true)
          Text(message).accessibilityIdentifier("answer-feedback")
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
        .frame(maxWidth: .infinity)
      }
    }
    .sheet(isPresented: $showingModelLibrary) {
      ModelLibraryView(
        store: modelLibrary,
        inferenceDirectory: inferenceDirectory,
        backendReady: connection == .ready,
        restartBlocked: store.answerBusy || store.modelSwitching
      ) { restart() }
    }
    .task(id: connection == .ready) {
      if connection == .ready { await store.send(.loadModels).finish() }
    }
  }

  private func traceButton(_ id: String) -> some View {
    Button("View Trace", systemImage: "waveform.path") { showTrace(id) }
      .accessibilityIdentifier("view-answer-trace")
  }
}
