import LocalAPI
import SwiftUI

struct AnswerResult: View {
  let answer: Components.Schemas.LocalAnswer
  let viewTrace: (() -> Void)?

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack {
        Text("Answer").font(.headline).accessibilityAddTraits(.isHeader)
        Spacer()
        if let viewTrace {
          Button("View Trace", systemImage: "waveform.path", action: viewTrace)
            .accessibilityIdentifier("view-answer-trace")
        }
      }
      Text(answer.text ?? "The local corpus has insufficient evidence.")
        .font(.body)
        .lineSpacing(4)
        .textSelection(.enabled)
    }
    if !answer.citations.isEmpty {
      WorkspaceGroup(title: "Sources") {
        VStack(alignment: .leading, spacing: 8) {
          ForEach(Array(answer.citations.enumerated()), id: \.offset) { offset, citation in
            if offset > 0 { Divider() }
            VStack(alignment: .leading, spacing: 5) {
              HStack(spacing: 8) {
                Image(systemName: "doc.text").foregroundStyle(.secondary).accessibilityHidden(true)
                Text("\(citation.evidence.source) @ \(citation.evidence.revision)")
                  .accessibilityIdentifier("answer-citation")
              }
              .font(.system(.callout, design: .monospaced))
              Text(citation.quote)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
          }
        }
      }
    }
  }
}
