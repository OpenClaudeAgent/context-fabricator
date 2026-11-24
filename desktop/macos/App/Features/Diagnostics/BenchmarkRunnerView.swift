import LocalAPI
import SwiftUI

struct BenchmarkRunnerView: View {
  struct Question: Equatable {
    let query: String
    let expected: String?
    let action: Components.Schemas.BenchmarkAction?
    var sourceLabel: String?
    var available = true
    var sourceURL: URL?

    var referenceURL: URL? {
      guard let sourceURL, sourceURL.scheme == "https", sourceURL.host == "github.com",
        sourceURL.user == nil, sourceURL.password == nil, sourceURL.port == nil
      else { return nil }
      return sourceURL
    }

    var referenceLabel: String? {
      if let url = referenceURL {
        let parts = url.pathComponents.dropFirst()
        if parts.count > 4, parts.dropFirst(2).first == "blob" {
          return parts.dropFirst(4).joined(separator: "/")
        }
      }
      return sourceLabel
    }
  }

  struct Suite: Identifiable {
    let id: String
    let title: String
    let ready: Bool
    let reference: Components.Schemas.BenchmarkReference?
    let questions: [Question]
    var documents: [Components.Schemas.BenchmarkDocument]?
  }

  private struct ResultRow: Identifiable {
    let id: Int
    let question: Question
    let response: AnswerResponse?
    let error: String?
  }

  let suites: [Suite]
  let preferredReference: Components.Schemas.BenchmarkReference?
  let supervisor: BackendSupervisor
  let isSelected: Bool
  @Binding var running: Bool
  let manageSources: () -> Void
  let corpusSaved: (Components.Schemas.BenchmarkReference) -> Bool
  let activateCorpus: (Components.Schemas.BenchmarkReference) -> Void
  let loadingCorpus: Bool
  let inspect: (Suite, Question, AnswerResponse) -> Void

  @State private var selectedID: String?
  @State private var mode: Components.Schemas.BenchmarkQuestion.retrievalPayload = .hybrid
  @State private var rows: [ResultRow] = []
  @State private var batch: Set<Int> = []
  @State private var stopping = false
  @State private var embeddingProgress: Components.Schemas.RetrievalProgress?
  @State private var status: String?
  @State private var hybridUnavailable = false
  @State private var selectedRow: Int?
  @State private var activeRequest: UUID?

  private var suite: Suite? {
    suites.first { $0.id == selectedID }
      ?? suites.first { preferredReference != nil && $0.reference == preferredReference }
      ?? suites.first(where: \.ready) ?? suites.first
  }
  private var selectedQuestion: Question? {
    displayedRows.first { $0.id == selectedRow }?.question
  }
  private var displayedRows: [ResultRow] {
    guard let suite else { return [] }
    let completed = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0) })
    return suite.questions.enumerated().map { index, question in
      completed[index] ?? .init(id: index, question: question, response: nil, error: nil)
    }
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack {
        if let suite {
          Picker(
            "Question Set", selection: Binding(get: { suite.id }, set: { selectedID = $0 })
          ) {
            ForEach(suites) { option in
              Text(verbatim: option.title).tag(option.id)
            }
          }
          .disabled(running)
        }
        if suite?.reference != nil {
          Picker("Search mode", selection: $mode) {
            Text("Hybrid").tag(Components.Schemas.BenchmarkQuestion.retrievalPayload.hybrid)
            Text("BM25").tag(Components.Schemas.BenchmarkQuestion.retrievalPayload.bm25)
          }
          .fixedSize()
          .disabled(running)
        }
        Spacer(minLength: 16)
        Button("Run Question", systemImage: "play.fill") {
          if let selectedRow { run(only: selectedRow) }
        }
        .buttonStyle(.borderedProminent)
        .disabled(
          running || loadingCorpus || suite?.ready != true || selectedQuestion?.available != true)
        Button("Run All") { run() }
          .disabled(running || loadingCorpus || suite?.ready != true)
          .accessibilityIdentifier("benchmark-run-suite")
      }
      .padding(14)
      if let suite {
        HStack {
          if suite.ready {
            Label("Ready", systemImage: "checkmark.circle").foregroundStyle(.green)
          } else {
            Label("Activate the source for this question set.", systemImage: "tray")
              .foregroundStyle(.secondary)
              .lineLimit(1)
            Spacer()
            if loadingCorpus {
              ProgressView().controlSize(.small)
            }
            if let reference = suite.reference {
              Button(corpusSaved(reference) ? "Activate" : "Download Corpus") {
                activateCorpus(reference)
              }
              .disabled(loadingCorpus)
            } else {
              Button("Sources…", action: manageSources).disabled(loadingCorpus)
            }
          }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: 26)
        .padding(.horizontal, 14).padding(.bottom, 8)
      }
      Divider()
      if displayedRows.isEmpty {
        ContentUnavailableView("Benchmark Results", systemImage: "checklist")
          .frame(maxWidth: .infinity, maxHeight: .infinity)
      } else {
        HSplitView {
          List(displayedRows, selection: $selectedRow) { row in
            VStack(alignment: .leading, spacing: 4) {
              Text(row.question.query).lineLimit(2, reservesSpace: true)
              Text(verbatim: row.question.referenceLabel ?? "")
                .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                .frame(height: 14, alignment: .leading)
                .accessibilityHidden(row.question.referenceLabel == nil)
              ZStack(alignment: .leading) {
                if row.question.available {
                  outcome(row)
                } else {
                  Text("Source unavailable").foregroundStyle(.secondary)
                }
              }
              .font(.caption)
              .frame(height: 14, alignment: .leading)
            }
            .padding(.vertical, 4)
            .tag(row.id)
          }
          .frame(minWidth: 265, idealWidth: 310)
          ScrollView {
            if let row = displayedRows.first(where: { $0.id == selectedRow }) {
              VStack(alignment: .leading, spacing: 16) {
                Text(row.question.query).font(.title3.bold()).textSelection(.enabled)
                if let url = row.question.referenceURL {
                  Link("Reference source", destination: url)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                }
                if let expected = row.question.expected {
                  resultSection("Expected Answer", expected)
                }
                if let answer = row.response?.answer, let text = answer.text {
                  resultSection("Model Answer", text)
                } else if let error = row.error {
                  resultSection("Error", error)
                } else if let reason = row.response?.answer.evidenceReview?.reason {
                  resultSection("Answer Outcome", reason.rawValue)
                }
                if let trace = row.response?.traceID { resultSection("Trace ID", trace) }
                if let suite, let response = row.response {
                  Button("Inspect Result") { inspect(suite, row.question, response) }
                    .disabled(running)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                }
              }
              .padding(18)
              .frame(maxWidth: .infinity, alignment: .leading)
            }
          }
          .frame(minWidth: 350)
        }
      }
      Divider()
      HStack(spacing: 10) {
        if hybridUnavailable {
          Label(
            "Hybrid requires a working embedding model. Check Models in Settings.",
            systemImage: "exclamationmark.circle"
          )
          .foregroundStyle(.orange).lineLimit(1)
          .help("Hybrid requires a working embedding model. Check Models in Settings.")
          if mode == .hybrid { Button("Use BM25") { mode = .bm25 } }
        } else if let status {
          Text(verbatim: status).foregroundStyle(.secondary).lineLimit(1).help(status)
        }
        Spacer()
        if running {
          let completed = rows.filter { batch.contains($0.id) }.count
          if let progress = embeddingProgress, progress.total > 0 {
            ProgressView(value: Double(progress.indexed), total: Double(progress.total))
              .frame(width: 112)
              .accessibilityLabel("Preparing benchmark embeddings…")
          } else {
            ProgressView(value: Double(completed), total: Double(batch.count))
              .frame(width: 112)
              .accessibilityLabel("Running benchmark…")
          }
          Text("\(completed) of \(batch.count) questions").monospacedDigit()
          Button("Stop", action: stop).disabled(stopping)
        } else if !rows.isEmpty {
          let published = rows.filter { $0.response?.answer.outcome == .answered }.count
          Text("\(published) published · \(rows.count) completed")
        } else if let suite {
          Text("\(suite.questions.count) questions")
        }
      }
      .lineLimit(1)
      .padding(.horizontal, 14)
      .frame(height: 44)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .onChange(of: isSelected, initial: true) { _, _ in selectAvailableSuite() }
    .onChange(of: suites.map(\.id)) { _, _ in selectAvailableSuite() }
    .onChange(of: suite?.questions, initial: true) { _, _ in
      rows = []
      selectedRow =
        suite?.questions.firstIndex(where: \.available)
        ?? suite?.questions.indices.first
      status = nil
      hybridUnavailable = false
    }
  }

  private func selectAvailableSuite() {
    guard isSelected else { return }
    selectedID = suite?.id
  }

  @ViewBuilder private func outcome(_ row: ResultRow) -> some View {
    if row.error != nil {
      Label("Error", systemImage: "xmark.circle").foregroundStyle(.red)
    } else if row.response == nil {
      if running && batch.contains(row.id) {
        Label("Queued", systemImage: "clock").foregroundStyle(.secondary)
      }
    } else if row.response?.answer.outcome == .answered {
      Label("Published", systemImage: "checkmark.circle").foregroundStyle(.green)
    } else {
      Label("Insufficient evidence", systemImage: "exclamationmark.circle").foregroundStyle(.orange)
    }
  }

  private func resultSection(_ title: LocalizedStringKey, _ content: String) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      Text(title).font(.headline)
      Text(verbatim: content).textSelection(.enabled)
    }
  }

  private func run(only index: Int? = nil) {
    guard let suite, suite.ready, !running else { return }
    let available = suite.questions.indices.filter { suite.questions[$0].available }
    let indices = index.map { available.contains($0) ? [$0] : [] } ?? available
    guard !indices.isEmpty else { return }
    let retrieval = mode
    batch = Set(indices)
    running = true
    stopping = false
    status = nil
    hybridUnavailable = false
    rows.removeAll { batch.contains($0.id) }
    Task { await run(suite, indices: indices, retrieval: retrieval) }
  }

  private func run(
    _ suite: Suite, indices: [Int], retrieval: Components.Schemas.BenchmarkQuestion.retrievalPayload
  ) async {
    defer {
      running = false
      stopping = false
      activeRequest = nil
      embeddingProgress = nil
    }
    do {
      let session = try await supervisor.connect(startIfNeeded: false)
      if let reference = suite.reference, retrieval == .hybrid {
        while !stopping {
          let progress = try await session.embedBenchmarkCorpusBatch(reference)
          embeddingProgress = progress
          if progress.state == .ready { break }
        }
        embeddingProgress = nil
      }
      for index in indices {
        if stopping { break }
        await runQuestion(at: index, in: suite, retrieval: retrieval, session: session)
        if selectedRow == nil { selectedRow = rows.first?.id }
      }
      if stopping { status = String(localized: "Benchmark stopped.") }
    } catch {
      hybridUnavailable =
        suite.reference != nil && retrieval == .hybrid
        && PipelineBenchmark.embeddingUnavailable(error)
      status = error.localizedDescription
    }
  }

  private func runQuestion(
    at index: Int, in suite: Suite,
    retrieval: Components.Schemas.BenchmarkQuestion.retrievalPayload,
    session: SessionClient
  ) async {
    let question = suite.questions[index]
    let id = UUID()
    activeRequest = id
    defer { activeRequest = nil }
    let row: ResultRow
    do {
      let response = try await session.askBenchmark(
        question.query, id: id, documents: suite.documents, reference: suite.reference,
        action: question.action, retrieval: suite.reference == nil ? nil : retrieval)
      row = .init(id: index, question: question, response: response, error: nil)
    } catch {
      row = .init(id: index, question: question, response: nil, error: error.localizedDescription)
    }
    if !stopping { rows.append(row) }
  }

  private func stop() {
    stopping = true
    guard let activeRequest else { return }
    Task {
      do {
        try await supervisor.connect(startIfNeeded: false).cancelAnswer(activeRequest)
      } catch {
        // The current request may already have finished. The next question stays stopped.
      }
    }
  }
}
