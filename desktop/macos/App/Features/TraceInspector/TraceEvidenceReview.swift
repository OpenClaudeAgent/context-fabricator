import LocalAPI
import SwiftUI

struct TraceEvidenceReview: View {
  @Environment(\.locale) private var locale
  typealias Review = Components.Schemas.AnswerEvidenceReview
  typealias Candidate = Components.Schemas.AnswerEvidenceCandidate
  typealias Search = Components.Schemas.AnswerEvidenceSearch

  let review: Review
  let mode: Components.Schemas.LocalAnswer.modePayload

  var body: some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 18) {
        TraceWithheldSummary(detail: review.explanation(in: locale))
        VStack(alignment: .leading, spacing: 4) {
          sectionHeading("Question")
          Text(verbatim: review.question).font(.body).textSelection(.enabled)
            .accessibilityIdentifier("trace-evidence-question")
        }
        searchedSources
        candidates
      }
      .padding(20)
      .frame(maxWidth: .infinity, alignment: .topLeading)
    }
  }

  private var searchedSources: some View {
    VStack(alignment: .leading, spacing: 8) {
      sectionHeading("Sources searched")
      if let scope = review.scope {
        Text(
          "Repository \(scope.repositoryId.formatted(.number.grouping(.never))) · \(scope.branch)"
        )
        .font(.callout).foregroundStyle(.secondary)
      }
      VStack(spacing: 0) {
        ForEach(Array(review.searches.enumerated()), id: \.offset) { index, search in
          if index > 0 { Divider() }
          sourceRow(search, index: index)
        }
      }
      .padding(.horizontal, 12)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 8))
    }
  }

  private func sourceRow(_ search: Search, index: Int) -> some View {
    let detail = search.selection ?? (search.source == .localPassages ? searchMethod : nil)
    return HStack(alignment: .firstTextBaseline, spacing: 10) {
      Image(systemName: sourceIcon(search.source))
        .foregroundStyle(.secondary)
        .frame(width: 18)
        .accessibilityHidden(true)
      VStack(alignment: .leading, spacing: 3) {
        Text(sourceName(search.source)).fontWeight(.medium)
        Text(verbatim: detail ?? "")
          .font(.caption)
          .foregroundStyle(.secondary)
          .lineLimit(1, reservesSpace: true)
          .truncationMode(.middle)
          .textSelection(.enabled)
          .help(detail ?? "")
          .accessibilityIdentifier("trace-evidence-source-detail-\(index)")
          .accessibilityHidden(detail == nil)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      Text("\(search.candidateCount) candidates")
        .monospacedDigit()
        .foregroundStyle(.secondary)
        .fixedSize()
        .accessibilityIdentifier("trace-evidence-source-count-\(index)")
    }
    .padding(.vertical, 8)
  }

  private var candidates: some View {
    VStack(alignment: .leading, spacing: 8) {
      sectionHeading("Candidates")
      if review.candidates.isEmpty {
        Text("There were no candidates to review.")
          .font(.callout).foregroundStyle(.secondary)
          .accessibilityIdentifier("trace-evidence-no-candidates")
      } else {
        ForEach(Array(review.candidates.enumerated()), id: \.offset) { index, candidate in
          if index > 0 { Divider() }
          candidateRow(candidate, index: index)
        }
      }
    }
  }

  private func candidateRow(_ candidate: Candidate, index: Int) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      Text(verbatim: candidate.reference).font(.body.weight(.medium)).textSelection(.enabled)
        .accessibilityIdentifier("trace-evidence-candidate-\(index)")
      Text(candidateLocation(candidate)).font(.caption).foregroundStyle(.secondary)
      if let preview = candidate.preview, !preview.isEmpty {
        Text(verbatim: preview)
          .font(.callout.monospaced()).textSelection(.enabled)
          .padding(9)
          .frame(maxWidth: .infinity, alignment: .leading)
          .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 6))
      }
      Label(candidate.disposition.explanation(in: locale), systemImage: "info.circle")
        .font(.callout).foregroundStyle(.secondary)
        .accessibilityIdentifier("trace-evidence-disposition-\(index)")
    }
  }

  private func candidateLocation(_ candidate: Candidate) -> String {
    let lines = candidate.startLine.map { start in
      candidate.endLine.map {
        $0 == start ? locale.localized("Line \(start)") : locale.localized("Lines \(start)–\($0)")
      }
        ?? locale.localized("Line \(start)")
    }
    return [
      sourceName(candidate.source), candidate.revision, lines,
      candidate.rank.map { locale.localized("Search rank \($0)") },
      candidate.slot.map { locale.localized("Proposed input \($0)") },
    ].compactMap { $0 }.joined(separator: " · ")
  }

  private var searchMethod: String {
    switch mode {
    case .bm25: locale.localized("Keyword search (BM25)")
    case .semantic: locale.localized("Semantic search")
    case .hybrid: locale.localized("Keyword and semantic search")
    case .selected: locale.localized("Selected source")
    }
  }

  private func sourceName(_ source: Components.Schemas.AnswerEvidenceSource) -> String {
    switch source {
    case .localPassages: locale.localized("Local passage index")
    case .githubHistory: locale.localized("GitHub change history")
    case .githubActions: locale.localized("GitHub Actions attempts")
    }
  }

  private func sourceIcon(_ source: Components.Schemas.AnswerEvidenceSource) -> String {
    switch source {
    case .localPassages: "doc.text.magnifyingglass"
    case .githubHistory: "clock.arrow.circlepath"
    case .githubActions: "play.square"
    }
  }

  private func sectionHeading(_ label: LocalizedStringKey) -> some View {
    Text(label).font(.headline).accessibilityAddTraits(.isHeader)
  }
}

struct TraceWithheldSummary: View {
  let detail: String

  var body: some View {
    HStack(alignment: .top, spacing: 11) {
      Image(systemName: "exclamationmark.circle.fill")
        .font(.title2).foregroundStyle(.orange)
        .accessibilityHidden(true)
      VStack(alignment: .leading, spacing: 5) {
        Text("Insufficient evidence")
          .font(.title3.weight(.semibold))
          .accessibilityAddTraits(.isHeader)
          .accessibilityIdentifier("trace-insufficient-evidence")
        Text(detail).foregroundStyle(.secondary)
          .accessibilityIdentifier("trace-evidence-reason")
      }
    }
    .padding(13)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(Color.orange.opacity(0.10), in: .rect(cornerRadius: 10))
  }
}

struct TraceRetrievalCounts: View {
  let counts: Components.Schemas.AnswerRetrievalCounts

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      Text("Recorded retrieval").font(.headline).accessibilityAddTraits(.isHeader)
      if let passages = counts.passageCandidates {
        LabeledContent("Ranked local passages", value: passages.formatted())
      }
      if let pairs = counts.historyPairs {
        LabeledContent("GitHub history pairs", value: pairs.formatted())
      }
      if let attempts = counts.workflowAttempts {
        LabeledContent("GitHub Actions attempts", value: attempts.formatted())
      }
      Divider()
      LabeledContent("Proposed for generation", value: counts.proposedSources.formatted())
        .accessibilityIdentifier("trace-recorded-proposed-sources")
    }
    .padding(12)
    .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 8))
  }
}

extension Components.Schemas.AnswerEvidenceCandidate.dispositionPayload {
  func explanation(in locale: Locale) -> String {
    switch self {
    case .candidateLimit: locale.localized("Outside the selection limit.")
    case .passageUnavailable:
      locale.localized("The indexed passage could not be reopened in the searched scope.")
    case .fixtureExcluded:
      locale.localized("The reformulated search excluded test fixtures for this question.")
    case .contextLimit: locale.localized("Excluded by the model’s input budget.")
    case .noUsableDraft:
      locale.localized("Proposed as evidence. The model returned no usable draft.")
    case .notCited: locale.localized("Sent to the model. The draft did not cite this source.")
    case .citationUnverified:
      locale.localized("Cited by the draft. Verification failed for the draft’s citations.")
    case .citationVerified:
      locale.localized("Cited by the draft and verified against the indexed source.")
    case .evidenceChanged: locale.localized("The evidence changed before verification finished.")
    }
  }
}
