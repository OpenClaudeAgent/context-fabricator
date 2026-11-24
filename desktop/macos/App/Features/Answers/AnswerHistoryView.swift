import ComposableArchitecture
import DesktopFeature
import SwiftUI

struct AnswerHistoryView: View {
  @Environment(\.locale) private var locale
  private enum Pane: Hashable { case session, saved }

  let store: StoreOf<AnswerHistory>
  let answers: StoreOf<Answers>
  let showSettings: () -> Void
  let showTrace: (String) -> Void
  @State private var pane: Pane
  @Environment(\.dismiss) private var dismiss

  init(
    store: StoreOf<AnswerHistory>, answers: StoreOf<Answers>,
    showSettings: @escaping () -> Void, showTrace: @escaping (String) -> Void
  ) {
    self.store = store
    self.answers = answers
    self.showSettings = showSettings
    self.showTrace = showTrace
    _pane = State(
      initialValue: !answers.sessionQuestions.isEmpty && store.confirmed?.enabled != true
        ? .session : .saved)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack {
        Text("Answer History").font(.title2.bold())
        Spacer()
        Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
      }
      Picker("History type", selection: $pane) {
        Text("This Session").tag(Pane.session)
        Text("Saved").tag(Pane.saved)
      }
      .pickerStyle(.segmented)
      .labelsHidden()
      .fixedSize()
      .accessibilityIdentifier("answer-history-type")
      VStack(alignment: .leading, spacing: 10) {
        if pane == .session {
          sessionHistory
        } else {
          savedHistory
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
    .padding(20)
    .frame(width: 640, height: 400)
    .task {
      await store.send(.loadSettings).finish()
      if store.confirmed?.enabled != false { await store.send(.reload).finish() }
    }
    .onDisappear { store.send(.discardCachedEntries) }
  }

  private var sessionHistory: some View {
    VStack(alignment: .leading, spacing: 10) {
      if answers.sessionQuestions.isEmpty {
        historyPlaceholder(
          "No Questions This Session", icon: "bubble.left.and.text.bubble.right",
          description: "Submit a question in Answers to see it here.")
      } else {
        Text("Recent questions from this app session. They disappear when you quit.")
          .foregroundStyle(.secondary)
        List(answers.sessionQuestions) { question in
          Button {
            showTrace(question.id)
          } label: {
            HStack {
              VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: question.text).lineLimit(2)
                Text(sessionOutcome(question))
                  .font(.caption).foregroundStyle(.secondary)
              }
              Spacer()
              Image(systemName: "chevron.right").foregroundStyle(.secondary)
                .accessibilityHidden(true)
            }
            .padding(.vertical, 4)
            .contentShape(.rect)
          }
          .buttonStyle(.plain)
          .accessibilityLabel(Text(verbatim: question.text))
          .accessibilityValue(sessionOutcome(question))
          .accessibilityIdentifier("session-question-\(question.id)")
        }
        .listStyle(.inset)
      }
      if store.confirmed?.enabled == false {
        HStack {
          Text("Saving history is off.").foregroundStyle(.secondary)
          Spacer()
          settingsButton
        }
      }
    }
  }

  @ViewBuilder private var savedHistory: some View {
    if store.confirmed?.enabled == false {
      historyPlaceholder(
        "Saving History Is Off", icon: "clock",
        description: "Enable answer history to keep questions and answers after quitting.",
        showsSettings: true)
    } else if let feedback = store.feedback ?? store.settingsFeedback, store.entries.isEmpty,
      store.confirmed == nil || store.feedback != nil
    {
      Label(locale.localizedKey(feedback), systemImage: "exclamationmark.circle")
        .foregroundStyle(.secondary)
    } else if store.confirmed == nil || (store.loading && store.entries.isEmpty) {
      LoadingStateView("Loading history…")
    } else if store.entries.isEmpty {
      historyPlaceholder(
        "No saved answers.", icon: "clock", description: "New completed questions will appear here."
      )
    } else {
      Text("Saved locally. Viewing an entry does not ask the model again.")
        .foregroundStyle(.secondary)
      savedEntries
    }
  }

  private func historyPlaceholder(
    _ title: LocalizedStringKey, icon: String, description: LocalizedStringKey,
    showsSettings: Bool = false
  ) -> some View {
    VStack(spacing: 8) {
      Image(systemName: icon)
        .font(.title2)
        .foregroundStyle(.secondary)
        .accessibilityHidden(true)
      Text(title).font(.headline).accessibilityAddTraits(.isHeader)
      Text(description).font(.callout).foregroundStyle(.secondary)
      if showsSettings { settingsButton.padding(.top, 4) }
    }
    .multilineTextAlignment(.center)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }

  private var settingsButton: some View {
    Button("History Settings…", systemImage: "gearshape", action: showSettings)
      .accessibilityIdentifier("open-answer-history-settings")
  }

  private func sessionOutcome(_ question: Answers.SessionQuestion) -> String {
    switch question.outcome {
    case .answered: locale.localized("Answered")
    case .insufficientEvidence: locale.localized("Insufficient evidence")
    case nil: locale.localized("Request ended")
    }
  }

  private var savedEntries: some View {
    HSplitView {
      ScrollView {
        LazyVStack(alignment: .leading, spacing: 8) {
          ForEach(store.entries, id: \.id) { entry in
            Button {
              store.send(.select(entry.id))
            } label: {
              VStack(alignment: .leading, spacing: 3) {
                Text(entry.question).lineLimit(2)
                Text(
                  Date(timeIntervalSince1970: Double(entry.completedAtMs) / 1000), style: .date
                )
                .font(.caption).foregroundStyle(.secondary)
              }
              .frame(maxWidth: .infinity, alignment: .leading)
              .padding(6)
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("saved-answer-\(entry.id)")
          }
          if store.nextBeforeId != nil { loadMoreRow }
        }.padding(8)
      }.frame(minWidth: 200, idealWidth: 240)
      savedDetail.frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
    }
  }

  private var loadMoreRow: some View {
    ZStack {
      Button("Load More") { store.send(.loadMore) }
        .opacity(store.loading ? 0 : 1)
        .disabled(store.loading)
        .accessibilityHidden(store.loading)
      if store.loading {
        ProgressView().controlSize(.small).accessibilityLabel("Loading more history")
      }
    }
    .frame(maxWidth: .infinity)
  }

  @ViewBuilder private var savedDetail: some View {
    if store.loadingDetail {
      LoadingStateView("Loading saved answer…")
    } else if let entry = store.selected {
      ScrollView {
        VStack(alignment: .leading, spacing: 12) {
          Text(entry.question)
            .font(.title3.bold())
            .textSelection(.enabled)
            .accessibilityIdentifier("saved-answer-question")
          Text(Date(timeIntervalSince1970: Double(entry.completedAtMs) / 1000), style: .date)
            .foregroundStyle(.secondary)
          if let repository = entry.scope.repositoryId {
            Text("Repository \(repository)").font(.caption)
          }
          if let branch = entry.scope.branch { Text("Branch: \(branch)").font(.caption) }
          if let commit = entry.scope.historyCommit {
            Text("Commit: \(commit)").font(.caption.monospaced())
          }
          if let run = entry.scope.actionsRunId {
            Text("Actions run: \(run)").font(.caption)
            if let attempt = entry.scope.actionsRunAttempt {
              Text("Attempt: \(attempt)").font(.caption)
            }
          }
          AnswerResult(
            answer: entry.answer,
            viewTrace: entry.traceId.map { id in { showTrace(id) } })
        }.frame(maxWidth: .infinity, alignment: .leading).padding(12)
      }
    } else {
      Text("Select an answer to view it.").foregroundStyle(.secondary)
    }
  }
}
