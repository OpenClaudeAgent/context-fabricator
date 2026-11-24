import ComposableArchitecture
import DesktopFeature
import SwiftUI

struct AnswerComposer: View {
  let store: StoreOf<Answers>
  let connection: Dashboard.Connection
  let restart: () -> Void
  let showModelLibrary: () -> Void
  @FocusState private var editingQuestion: Bool

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      Text("Question").font(.headline).accessibilityAddTraits(.isHeader)
      VStack(spacing: 0) {
        editor
        Divider()
        footer
          .padding(.horizontal, 12)
          .padding(.vertical, 9)
          .background(.bar)
      }
      .background(.background)
      .clipShape(.rect(cornerRadius: 10))
      .overlay {
        RoundedRectangle(cornerRadius: 10)
          .strokeBorder(
            editingQuestion ? Color.accentColor.opacity(0.65) : Color.secondary.opacity(0.25),
            lineWidth: editingQuestion ? 1.5 : 1
          )
          .allowsHitTesting(false)
      }
      if let feedback = store.modelFeedback {
        Label(feedback, systemImage: "exclamationmark.circle")
          .font(.callout)
          .foregroundStyle(.secondary)
      }
    }
  }

  private var editor: some View {
    TextEditor(
      text: Binding(get: { store.question }, set: { store.send(.questionChanged($0)) })
    )
    .font(.system(size: 15))
    .scrollContentBackground(.hidden)
    .padding(.horizontal, 11)
    .padding(.vertical, 10)
    .frame(height: 148)
    .overlay(alignment: .topLeading) {
      if store.question.isEmpty {
        Text("Ask about your local sources…")
          .font(.system(size: 15))
          .foregroundStyle(.tertiary)
          .padding(.leading, 16)
          .padding(.top, 10)
          .accessibilityHidden(true)
          .allowsHitTesting(false)
      }
    }
    .focused($editingQuestion)
    .accessibilityLabel("Question")
    .accessibilityIdentifier("answer-question")
    .onKeyPress(.return, phases: .down) { press in
      guard !press.modifiers.contains(.shift), canAsk else { return .ignored }
      store.send(.ask)
      return .handled
    }
  }

  private var footer: some View {
    ViewThatFits(in: .horizontal) {
      HStack(spacing: 12) {
        modelControls
        Spacer(minLength: 4)
        sendControls
      }
      VStack(alignment: .leading, spacing: 8) {
        modelControls
        sendControls.frame(maxWidth: .infinity, alignment: .trailing)
      }
    }
  }

  private var modelControls: some View {
    AnswerModelControls(
      store: store, connection: connection, restart: restart, showModelLibrary: showModelLibrary)
  }

  private var sendControls: some View {
    answerAction.fixedSize()
  }

  private var answerAction: some View {
    Group {
      if store.answerBusy {
        Button("Cancel answer") { store.send(.cancelAnswer) }.disabled(store.answerCancelling)
      } else {
        Button {
          store.send(.ask)
        } label: {
          HStack(spacing: 7) {
            Label("Ask locally", systemImage: "arrow.up")
            Text("↵").font(.callout.monospaced()).accessibilityHidden(true)
          }
        }
        .accessibilityLabel("Ask locally")
        .keyboardShortcut(.return, modifiers: .command)
        .help("Return to send. Shift-Return for a new line.")
        .buttonStyle(.borderedProminent)
        .disabled(!canAsk)
      }
    }
    .controlSize(.regular)
    .fixedSize()
  }

  private var canAsk: Bool {
    connection == .ready && store.canAsk
  }
}
