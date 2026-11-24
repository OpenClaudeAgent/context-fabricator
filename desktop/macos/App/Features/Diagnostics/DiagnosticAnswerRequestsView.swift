import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct DiagnosticAnswerRequestsView: View {
  let store: StoreOf<DiagnosticLogs>
  let answers: StoreOf<Answers>
  let openTrace: (String) -> Void

  private var records: [Components.Schemas.SignalSample] {
    store.records.filter {
      $0.level != .debug && $0.traceId != nil && $0.traceId != answers.answerTraceID
    }
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      if let id = answers.answerTraceID {
        latestRequest(id)
      } else if answers.answerBusy && (!records.isEmpty || store.nextBeforeId != nil) {
        answerInProgress.frame(height: 56)
      }
      if let feedback = store.feedback {
        Label(feedback, systemImage: "exclamationmark.circle").foregroundStyle(.secondary)
      }
      if !records.isEmpty || store.nextBeforeId != nil || answers.answerTraceID == nil {
        requestHistory
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .task(id: answers.answerTraceID) {
      await store.send(.refresh).finish()
    }
  }

  private func latestRequest(_ id: String) -> some View {
    GroupBox {
      HStack(spacing: 12) {
        Image(systemName: "bubble.left.and.text.bubble.right")
          .foregroundStyle(Color.accentColor)
          .accessibilityHidden(true)
        VStack(alignment: .leading, spacing: 4) {
          Text("Latest Answer Request").font(.headline)
          if let question = answers.sessionQuestions.first(where: { $0.id == id })?.text
            ?? answers.answer?.evidenceReview?.question
            ?? answers.answer?.modelContext?.question
          {
            Text(verbatim: question)
              .lineLimit(2)
              .foregroundStyle(.secondary)
              .accessibilityIdentifier("diagnostic-latest-answer-question")
          }
          if answers.answer?.outcome == .insufficientEvidence {
            Label("Insufficient evidence", systemImage: "exclamationmark.circle")
              .font(.caption).foregroundStyle(.orange)
          }
        }
        Spacer(minLength: 0)
        Button("View Trace", systemImage: "waveform.path") { openTrace(id) }
          .buttonStyle(.borderedProminent)
          .accessibilityIdentifier("diagnostic-view-latest-answer-trace")
      }
      .padding(6)
    }
  }

  private var requestHistory: some View {
    List {
      Section("Recent Answer Requests") {
        ForEach(records, id: \.id) { record in
          if let id = record.traceId { requestRow(record, traceID: id) }
        }
        if store.nextBeforeId != nil {
          Button("Load Older Requests") { store.send(.loadMore) }
            .disabled(store.isLoading)
        }
      }
    }
    .listStyle(.inset)
    .scrollContentBackground(.hidden)
    .overlay {
      if records.isEmpty && store.nextBeforeId == nil {
        if answers.answerBusy {
          answerInProgress
        } else if store.isLoading {
          LoadingStateView("Loading requests…")
        } else if store.feedback == nil && answers.answerTraceID == nil && !answers.answerBusy {
          ContentUnavailableView(
            "No Answer Requests Yet", systemImage: "bubble.left.and.text.bubble.right",
            description: Text("Send a question in Answers or run ctxfab ask."))
        }
      }
    }
  }

  private var answerInProgress: some View {
    LoadingStateView("Answer request in progress…")
      .accessibilityIdentifier("diagnostic-answer-in-progress")
  }

  private func requestRow(_ record: Components.Schemas.SignalSample, traceID: String) -> some View {
    let failed = record.level == .error
    return HStack(spacing: 12) {
      Label(
        failed ? "Failed" : "Completed",
        systemImage: failed ? "exclamationmark.circle" : "checkmark.circle"
      )
      .foregroundStyle(failed ? Color.red : Color.green)
      Text(
        Date(timeIntervalSince1970: Double(record.timestamp)),
        format: .dateTime.month().day().hour().minute().second()
      )
      .foregroundStyle(.secondary)
      .monospacedDigit()
      if let question = answers.sessionQuestions.first(where: { $0.id == traceID }) {
        Text(verbatim: question.text).lineLimit(1).help(question.text)
      } else {
        Text(traceID.prefix(8))
          .font(.caption.monospaced())
          .foregroundStyle(.secondary)
          .help(traceID)
      }
      Spacer(minLength: 0)
      Button("View Trace") { openTrace(traceID) }
        .accessibilityIdentifier("diagnostic-request-trace-\(traceID)")
    }
  }
}
