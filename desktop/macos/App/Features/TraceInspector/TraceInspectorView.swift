import AppKit
import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct TraceInspectorView: View {
  @Environment(\.locale) private var locale
  private enum Pane: Hashable {
    case spans
    case logs
    case answer
  }

  let store: StoreOf<TraceInspector>
  let traceID: String
  let initialSpanID: String?
  let answers: StoreOf<Answers>
  @Environment(\.dismiss) private var dismiss
  @State private var pane: Pane
  @State private var selectedSpan: Int64?
  @State private var traceHistory: [String] = []

  private var currentTraceID: String { traceHistory.last ?? traceID }
  private var liveAnswer: Components.Schemas.LocalAnswer? {
    currentTraceID == answers.answerTraceID ? answers.answer : nil
  }
  private var archivedAnswer: Components.Schemas.AnswerHistoryEntry? {
    store.savedAnswer?.traceId == currentTraceID ? store.savedAnswer : nil
  }
  private var requestQuestion: String? {
    answers.sessionQuestions.first { $0.id == currentTraceID }?.text
      ?? liveAnswer?.evidenceReview?.question
      ?? liveAnswer?.modelContext?.question
      ?? archivedAnswer?.question
  }
  private var hasAnswerPane: Bool {
    liveAnswer != nil || requestQuestion != nil || store.state.isAnswerTrace(currentTraceID)
      || (pane == .answer && store.isLoading)
  }

  private var shouldRefreshSavedAnswer: Bool {
    hasAnswerPane && liveAnswer == nil && (pane == .answer || archivedAnswer != nil)
  }

  private var recordedAbstention: Bool {
    store.spans.contains { $0.spanOutcome == .insufficientEvidence }
  }
  private var recordedCounts: Components.Schemas.AnswerRetrievalCounts? {
    store.spans.first { $0.name == "answer_retrieval" }?.retrievalCounts
  }

  init(
    store: StoreOf<TraceInspector>, traceID: String, initialSpanID: String?,
    answers: StoreOf<Answers>
  ) {
    self.store = store
    self.traceID = traceID
    self.initialSpanID = initialSpanID
    self.answers = answers
    let hasAnswerDetails =
      initialSpanID == nil && traceID == answers.answerTraceID
      && (answers.answer?.modelContext != nil || answers.answer?.outcome == .insufficientEvidence)
    _pane = State(initialValue: hasAnswerDetails ? .answer : .spans)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header
      Divider()
      content
        .frame(maxWidth: .infinity, maxHeight: .infinity)
      footer
    }
    .frame(width: 800, height: 600)
    .task(id: currentTraceID) {
      let id = currentTraceID
      await store.send(.load(id)).finish()
      if id == currentTraceID, !Task.isCancelled,
        store.state.isAnswerTrace(id)
      {
        loadSavedAnswerIfNeeded()
      }
    }
    .onDisappear { store.send(.close) }
    .onReceive(NotificationCenter.default.publisher(for: .NSSystemClockDidChange)) { _ in
      if shouldRefreshSavedAnswer { store.send(.loadSavedAnswer) }
    }
    .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification))
    { _ in
      if shouldRefreshSavedAnswer { store.send(.loadSavedAnswer) }
    }
    .onChange(of: pane) { _, value in
      if value == .answer { loadSavedAnswerIfNeeded() }
    }
    .onChange(of: hasAnswerPane) { _, available in
      if !available, pane == .answer { pane = .spans }
    }
    .onChange(of: recordedAbstention) { _, withheld in
      if withheld, initialSpanID == nil, currentTraceID == traceID, pane == .spans {
        pane = .answer
      }
    }
  }

  private var header: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(spacing: 12) {
        if !traceHistory.isEmpty {
          Button("Back", systemImage: "chevron.left") {
            traceHistory.removeLast()
            showSpans()
          }
          .accessibilityIdentifier("trace-inspector-back")
        }
        Text(hasAnswerPane ? "Answer trace" : "Trace")
          .font(.title2.bold()).accessibilityAddTraits(.isHeader)
        Spacer()
        Button("Refresh Trace", systemImage: "arrow.clockwise") {
          let refreshSaved = shouldRefreshSavedAnswer
          store.send(.load(currentTraceID))
          if refreshSaved { store.send(.loadSavedAnswer) }
        }
        .labelStyle(.iconOnly)
        .help("Refresh Trace")
        .disabled(store.isLoading)
        Button("Close", systemImage: "xmark") { dismiss() }
          .keyboardShortcut(.cancelAction)
          .accessibilityIdentifier("trace-inspector-close")
      }
      HStack(spacing: 12) {
        Picker("Trace records", selection: $pane) {
          if hasAnswerPane {
            Text("Answer").tag(Pane.answer)
              .accessibilityIdentifier("trace-inspector-answer-pane")
          }
          Text("Spans").tag(Pane.spans)
          Text("Logs").tag(Pane.logs)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .accessibilityIdentifier("trace-inspector-pane")
        Spacer()
        traceIdentifier
      }
      if pane != .answer, let question = requestQuestion {
        VStack(alignment: .leading, spacing: 3) {
          Text("Question").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
          Text(verbatim: question).lineLimit(3).textSelection(.enabled).help(question)
            .accessibilityIdentifier("trace-request-question")
        }
        .frame(maxWidth: .infinity, alignment: .leading)
      }
    }
    .padding(.horizontal, 20)
    .padding(.vertical, 14)
  }

  private var traceIdentifier: some View {
    HStack(spacing: 6) {
      Text(currentTraceID).font(.caption.monospaced()).foregroundStyle(.secondary)
        .lineLimit(1)
        .truncationMode(.middle)
        .textSelection(.enabled)
        .accessibilityIdentifier("trace-inspector-id")
        .help(currentTraceID)
      Button("Copy Trace ID", systemImage: "document.on.document") {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(currentTraceID, forType: .string)
      }
      .labelStyle(.iconOnly)
      .buttonStyle(.borderless)
      .foregroundStyle(.secondary)
      .help("Copy Trace ID")
    }
  }

  @ViewBuilder private var content: some View {
    if pane == .answer && hasAnswerPane {
      savedAnswer
    } else if store.hasRecords {
      switch pane {
      case .spans:
        TraceSpanOutline(
          spans: store.spans, logs: store.logs,
          initialSpanID: traceHistory.isEmpty ? initialSpanID : nil,
          selection: $selectedSpan,
          viewCollectionTrace: { id in
            guard id != currentTraceID else { return }
            traceHistory.append(id)
            showSpans()
          })
      case .logs: TraceLogsView(logs: store.logs)
      case .answer: EmptyView()
      }
    } else if store.isLoading {
      LoadingStateView("Loading trace…")
    } else {
      ContentUnavailableView {
        Label("Trace Unavailable", systemImage: "waveform.path")
      } description: {
        Text(
          store.feedback
            ?? locale.localized("This trace may still be recording or may have expired."))
      } actions: {
        Button("Retry") { store.send(.load(currentTraceID)) }
      }
    }
  }

  private func showSpans() {
    selectedSpan = nil
    pane = .spans
  }

  private func loadSavedAnswerIfNeeded() {
    if hasAnswerPane && liveAnswer == nil && store.savedAnswer == nil && !store.loadingSavedAnswer {
      store.send(.loadSavedAnswer)
    }
  }

  @ViewBuilder private var savedAnswer: some View {
    if let answer = liveAnswer, let review = answer.evidenceReview {
      TraceEvidenceReview(review: review, mode: answer.mode)
    } else if liveAnswer?.outcome == .insufficientEvidence {
      legacyWithheld(question: requestQuestion)
    } else if let answer = liveAnswer {
      liveAnswerContent(answer)
    } else if store.loadingSavedAnswer {
      LoadingStateView("Loading saved answer…")
    } else if archivedAnswer?.answer.outcome == .insufficientEvidence || recordedAbstention {
      legacyWithheld(question: requestQuestion)
    } else if let entry = archivedAnswer {
      ScrollView {
        VStack(alignment: .leading, spacing: 12) {
          Text(entry.question).font(.title3.bold()).textSelection(.enabled)
            .accessibilityIdentifier("trace-saved-answer-question")
          AnswerResult(answer: entry.answer, viewTrace: nil)
        }.frame(maxWidth: .infinity, alignment: .leading).padding(20)
      }
    } else if let question = requestQuestion {
      ScrollView {
        VStack(alignment: .leading, spacing: 12) {
          Text("Question").font(.headline)
          Text(verbatim: question).textSelection(.enabled)
            .accessibilityIdentifier("trace-request-question")
          Text("The question is still in this session. The answer is no longer available.")
            .foregroundStyle(.secondary)
          Button("Show recorded steps") { pane = .spans }
        }
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
      }
    } else {
      ContentUnavailableView {
        Label("Saved Answer Unavailable", systemImage: "text.bubble")
          .accessibilityIdentifier("trace-saved-answer-unavailable")
      } description: {
        Text(
          store.savedAnswerFeedback.map { locale.localizedKey($0) }
            ?? locale.localized(
              "No saved answer is available for this trace. It may have expired or been removed.")
        )
      } actions: {
        Button("Retry") { store.send(.loadSavedAnswer) }
      }
    }
  }

  private func legacyWithheld(question: String?) -> some View {
    let lookupFoundNoEvidence = store.spans.contains {
      $0.name == "answer_retrieval"
        && $0.events?.contains { $0.name == "No evidence found" } == true
    }
    return ScrollView {
      VStack(alignment: .leading, spacing: 14) {
        TraceWithheldSummary(
          detail: lookupFoundNoEvidence
            ? "The recorded retrieval step found no usable evidence."
            : "This request ended without a verified answer.")
        if let question {
          Text("Question").font(.headline)
          Text(verbatim: question).textSelection(.enabled)
            .accessibilityIdentifier("trace-request-question")
        } else {
          Text("The question was not retained for this trace.").foregroundStyle(.secondary)
        }
        if let recordedCounts {
          TraceRetrievalCounts(counts: recordedCounts)
        }
        Text("Candidate details were not retained for this trace.").foregroundStyle(.secondary)
        Button("Show recorded steps") { pane = .spans }
      }
      .padding(20)
      .frame(maxWidth: .infinity, alignment: .topLeading)
    }
  }

  private func liveAnswerContent(_ answer: Components.Schemas.LocalAnswer) -> some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 16) {
        if let question = requestQuestion {
          Text("Question").font(.headline).accessibilityAddTraits(.isHeader)
          Text(verbatim: question).textSelection(.enabled)
            .accessibilityIdentifier("trace-model-question")
          Divider()
        }
        AnswerResult(answer: answer, viewTrace: nil, citationsTitle: "Verified Citations")
        Divider()
        if let context = answer.modelContext {
          Text("Sources Given to the Model").font(.headline).accessibilityAddTraits(.isHeader)
          ForEach(context.sources, id: \.slot) { source in
            TraceModelSource(source: source)
          }
        } else {
          Text("Model context is unavailable for this answer.").foregroundStyle(.secondary)
        }
      }.frame(maxWidth: .infinity, alignment: .leading).padding(20)
    }
  }

  @ViewBuilder private var footer: some View {
    if pane != .answer, store.hasRecords, store.nextBeforeId != nil || store.feedback != nil {
      Divider()
      HStack {
        if let message = store.feedback {
          Label(message, systemImage: "exclamationmark.circle").foregroundStyle(.secondary)
        }
        Spacer()
        if store.nextBeforeId != nil {
          if store.isLoading {
            ProgressView().controlSize(.small).accessibilityLabel("Loading older records")
          }
          Button("Load Older Records") { store.send(.loadMore) }
            .disabled(store.isLoading)
        }
      }
      .padding(.horizontal, 20)
      .padding(.vertical, 12)
    }
  }
}

private struct TraceModelSource: View {
  @Environment(\.locale) private var locale
  let source: Components.Schemas.AnswerModelSource

  private var title: String {
    switch source.kind {
    case .current: locale.localized("Current code")
    case .history: locale.localized("History")
    case .workflow: locale.localized("GitHub Actions")
    }
  }

  var body: some View {
    WorkspaceGroup(title: "\(source.slot). \(title)") {
      VStack(alignment: .leading, spacing: 10) {
        if let repository = source.repository {
          Text([repository, source.branch].compactMap { $0 }.joined(separator: " · "))
            .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
        }
        if let change = source.change {
          VStack(alignment: .leading, spacing: 4) {
            Text("Before · \(change.beforePath) @ \(change.parent)")
            Text("After · \(change.afterPath) @ \(change.child)")
          }
          .font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
        }
        ForEach(Array(source.parts.enumerated()), id: \.offset) { index, part in
          if index > 0 { Divider() }
          VStack(alignment: .leading, spacing: 5) {
            let location = [
              part.side.map {
                $0 == .before ? locale.localized("Before") : locale.localized("After")
              }, part.path,
              part.revision.map { "@ \($0)" },
              part.startLine.map { start in
                part.endLine.map { locale.localized("Lines \(start)–\($0)") }
                  ?? locale.localized("Line \(start)")
              },
            ].compactMap { $0 }.joined(separator: " · ")
            if !location.isEmpty {
              Text(verbatim: location).font(.caption.monospaced())
                .foregroundStyle(.secondary).textSelection(.enabled)
            }
            Text(verbatim: part.text).font(.callout.monospaced()).textSelection(.enabled)
              .accessibilityIdentifier("trace-model-source-\(source.slot)-part-\(index)")
          }.frame(maxWidth: .infinity, alignment: .leading)
        }
      }
    }
  }
}
