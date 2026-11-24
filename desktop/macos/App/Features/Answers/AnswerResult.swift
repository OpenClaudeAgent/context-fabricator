import LocalAPI
import SwiftUI

struct AnswerResult: View {
  @Environment(\.locale) private var locale
  let answer: Components.Schemas.LocalAnswer
  let viewTrace: (() -> Void)?
  var citationsTitle: LocalizedStringKey = "Sources"

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
      Text(
        answer.text ?? answer.evidenceReview?.explanation(in: locale)
          ?? locale.localized("The local corpus has insufficient evidence.")
      )
      .font(.body)
      .lineSpacing(4)
      .textSelection(.enabled)
      .accessibilityIdentifier("answer-result-text")
    }
    if !answer.citations.isEmpty {
      WorkspaceGroup(title: citationsTitle) {
        VStack(alignment: .leading, spacing: 8) {
          ForEach(Array(answer.citations.enumerated()), id: \.offset) { offset, citation in
            if offset > 0 { Divider() }
            VStack(alignment: .leading, spacing: 5) {
              HStack(spacing: 8) {
                Image(systemName: "doc.text").foregroundStyle(.secondary).accessibilityHidden(true)
                if let url = citation.lineUrl.flatMap(githubURL),
                  let location = citation.evidence.location
                {
                  let anchor = url.fragment.map { "#\($0)" } ?? ""
                  Link(
                    "\(location.path)\(anchor) @ \(citation.evidence.revision.prefix(10))",
                    destination: url
                  )
                  .help("Open the cited lines on GitHub")
                  .accessibilityIdentifier("answer-citation")
                } else {
                  Text("\(citation.evidence.source) @ \(citation.evidence.revision)")
                    .accessibilityIdentifier("answer-citation")
                }
              }
              .font(.system(.callout, design: .monospaced))
              if let comparison = citation.comparison {
                Text(
                  "\(comparison.side == .before ? "Before" : "After") · \(comparison.child.prefix(10)) vs \(comparison.parent.prefix(10))"
                )
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
              }
              if let run = citation.workflowRun {
                HStack {
                  Text(
                    "Run \(run.runId) · attempt \(run.attempt) · reported head \(run.reportedHead.prefix(10))"
                  )
                  if let url = githubURL(run.runUrl) {
                    Link("View run", destination: url)
                  }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
              }
              Text(verbatim: citation.quote)
                .font(citation.evidence.location == nil ? .callout : .callout.monospaced())
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(10)
                .background(.quaternary, in: .rect(cornerRadius: 6))
                .accessibilityIdentifier("answer-citation-quote-\(offset)")
            }
            .frame(maxWidth: .infinity, alignment: .leading)
          }
        }
      }
    }
  }

  private func githubURL(_ value: String) -> URL? {
    guard let url = URL(string: value), url.scheme == "https", url.host == "github.com" else {
      return nil
    }
    return url
  }
}

extension Components.Schemas.AnswerEvidenceReview {
  func explanation(in locale: Locale) -> String {
    switch reason {
    case .noCandidates:
      locale.localized("The search returned zero candidates. Answer generation was skipped.")
    case .noUsableCandidates:
      locale.localized(
        "The search returned candidates, but none were available for answer generation.")
    case .contextLimit:
      locale.localized("None of the proposed sources fit in the model’s input budget.")
    case .generationDeadline:
      locale.localized("The model reached its time limit before returning a usable draft.")
    case .workerUnavailable:
      locale.localized("The local model was unavailable before returning a usable draft.")
    case .invalidModelOutput:
      locale.localized("The model’s output did not match the response format.")
    case .noCitations:
      locale.localized("The model returned a draft without citations. The draft was withheld.")
    case .citationsUnverified:
      locale.localized(
        "The draft’s citations did not pass verification against the indexed sources.")
    case .verified:
      locale.localized("Citations verified against the indexed sources.")
    case .evidenceChanged:
      locale.localized("The source evidence changed before its citations could be verified.")
    }
  }
}
