import AppKit
import DesktopFeature
import Foundation
import LocalAPI
import Observation
import SwiftUI
import UniformTypeIdentifiers

@Observable
@MainActor
final class AnswerPipelineSources {
  var loadedCorpora: [Components.Schemas.BenchmarkReference: Components.Schemas.BenchmarkCorpus] =
    [:]
  var savedCorpora: [Components.Schemas.BenchmarkReference: Components.Schemas.BenchmarkCorpus] =
    [:]
  var workflows = PipelineWorkflowSources()
  var recordedWorkflows = PipelineWorkflowSources()
  var downloadedWorkflows = PipelineWorkflowSources()
  var actionDownloads:
    [Components.Schemas.BenchmarkReference: Components.Schemas.BenchmarkActionDownload] = [:]
  var recordedSnapshots: [Components.Schemas.GitHubActions] = []
}

private struct PipelineSourcePreferences: Codable {
  var workflows: Set<String> = []
  var recordedWorkflows: Set<String>?
  var downloadedWorkflows: Set<String>?
  var loadedCaptures: Bool?
  var importedQuestions: PipelineBenchmark.Reference?
}

struct AnswerPipelineView: View {
  private typealias Repository = Components.Schemas.GitHubConfiguredRepository
  private typealias Answer = Components.Schemas.LocalAnswer
  private typealias Candidate = Components.Schemas.AnswerEvidenceCandidate
  private typealias LoadedCorpus = Components.Schemas.BenchmarkCorpus

  private enum Stage: String, CaseIterable {
    case corpus, index, retrieval, embedding, ranking, context, generation, verification

    var title: LocalizedStringKey {
      switch self {
      case .corpus: "Corpus"
      case .index: "Search Index"
      case .retrieval: "Retrieval"
      case .embedding: "Embeddings"
      case .ranking: "Ranking"
      case .context: "Model Input"
      case .generation: "Model Output"
      case .verification: "Citation Verification"
      }
    }

    var icon: String {
      switch self {
      case .corpus: "tray.full"
      case .index: "cylinder.split.1x2"
      case .retrieval: "magnifyingglass"
      case .embedding: "point.3.connected.trianglepath.dotted"
      case .ranking: "list.number"
      case .context: "text.alignleft"
      case .generation: "sparkles"
      case .verification: "checkmark.seal"
      }
    }

    var tint: Color {
      switch self {
      case .corpus, .index: .indigo
      case .retrieval, .embedding, .ranking: .blue
      case .context, .generation: .purple
      case .verification: .teal
      }
    }
  }

  private enum WorkflowOrigin: Hashable { case connected, recorded, downloaded }

  private struct InspectionRequest {
    let query: String
    let id: UUID
    let repository: Repository?
    let files: [Components.Schemas.BenchmarkDocument]
    let reference: Components.Schemas.BenchmarkReference?
    let workflow: PipelineWorkflow?
    let origin: WorkflowOrigin?
    let retrieval: Components.Schemas.BenchmarkQuestion.retrievalPayload
  }

  private enum CorpusScope: Hashable {
    case all
    case repository(Int64)
    case reference(Components.Schemas.BenchmarkReference)
    case workflow(PipelineWorkflow, origin: WorkflowOrigin)
    case temporary
  }

  private enum BenchmarkImport { case files, suite }
  private enum SourceManagerTab: Hashable {
    case corpora, publicActions, connectedActions, recordedActions

    var title: LocalizedStringKey {
      switch self {
      case .corpora: "Benchmark Corpora"
      case .publicActions: "Public Actions for Benchmark Corpora"
      case .connectedActions: "Connected Repositories"
      case .recordedActions: "Recorded Captures"
      }
    }
  }

  private struct SelectedExample {
    let question: PipelineBenchmark.Question
    let reference: PipelineBenchmark.Reference?
  }

  @Environment(\.dismiss) private var dismiss
  @Environment(\.locale) private var locale
  @State private var question: String
  @State private var scope: CorpusScope
  @State private var documents: [PipelineBenchmark.Document] = []
  @State private var customQuestions: [PipelineBenchmark.Question] = []
  @State private var importedReference: PipelineBenchmark.Reference?
  @State private var example: SelectedExample?
  @State private var showsDocumentEditor = false
  @State private var showsImporter = false
  @State private var benchmarkSelected = false
  @State private var benchmarkRunning = false
  @State private var importKind: BenchmarkImport = .files
  @State private var importFeedback: String?
  let sources: AnswerPipelineSources
  @State private var showsSources = false
  @State private var showsSourceManager = false
  @State private var sourceManagerTab: SourceManagerTab = .corpora
  @State private var showsFixtureCatalog = false
  @State private var sourceManagerFeedback: String?
  @State private var sourceManagerReport: String?
  @State private var sourceBatchStarted: Date?
  @State private var sourceBatchCurrent = ""
  @State private var pendingRepository: Int64?
  @State private var sourcePreferences = PipelineSourcePreferences()
  @State private var restoringSources = true
  @State private var loadingCorpus = false
  @State private var pendingReference: Components.Schemas.BenchmarkReference?
  @State private var repositories: [Repository]
  @State private var stage: Stage = .retrieval
  @State private var response: AnswerResponse?
  @State private var requestID: UUID?
  @State private var submittedQuestion = ""
  @State private var submittedRepository: Repository?
  @State private var submittedCorpus: LoadedCorpus?
  @State private var submittedWorkflow: PipelineWorkflow?
  @State private var submittedBenchmarkBM25 = false
  @State private var submittedFiles: [String] = []
  @State private var submittedBytes = 0
  @State private var feedback: String?
  @State private var hybridUnavailable = false
  @State private var setupFeedback: String?
  @State private var cancelling = false
  @State private var benchmarkRetrieval: Components.Schemas.BenchmarkQuestion.retrievalPayload =
    .hybrid
  @State private var indexingBenchmark = false
  @State private var indexProgress: Components.Schemas.RetrievalProgress?
  @State private var selectedCandidate: Int?
  @State private var expandedRetrievalCandidate: Int?
  @State private var focusedSlot: Int?
  @State private var expandedModelSlots: Set<Int> = []
  @FocusState private var composerFocused: Bool

  let supervisor: BackendSupervisor

  init(
    supervisor: BackendSupervisor, initialQuestion: String,
    initialRepository: Components.Schemas.GitHubConfiguredRepository?,
    sources: AnswerPipelineSources
  ) {
    self.supervisor = supervisor
    self.sources = sources
    _question = State(initialValue: initialQuestion)
    _repositories = State(initialValue: initialRepository.map { [$0] } ?? [])
    _scope = State(initialValue: initialRepository.map { .repository($0.repository.id) } ?? .all)
  }

  private var query: String { question.trimmingCharacters(in: .whitespacesAndNewlines) }
  private var answer: Answer? { response?.answer }
  private var review: Components.Schemas.AnswerEvidenceReview? { answer?.evidenceReview }
  private var capture: Components.Schemas.AnswerPipeline? { review?.pipeline }

  private var selectedRepository: Repository? {
    guard case .repository(let id) = scope else { return nil }
    return repositories.first { $0.repository.id == id }
  }

  private var selectedExample: SelectedExample? { example?.question.query == query ? example : nil }

  private var referenceCorpus: PipelineBenchmark.Reference? {
    guard case .reference(let reference) = scope else { return nil }
    return example?.reference.flatMap { $0.source == reference ? $0 : nil }
      ?? managedReferences.first { $0.source == reference }
  }

  private var readyReference: LoadedCorpus? {
    referenceCorpus.flatMap { sources.loadedCorpora[$0.source] }
  }

  private var selectedWorkflow: PipelineWorkflow? {
    guard case .workflow(let workflow, _) = scope else { return nil }
    return workflow
  }

  private var workflowOrigin: WorkflowOrigin? {
    if case .workflow(_, let origin) = scope { return origin }
    return nil
  }

  private func workflowSources(_ origin: WorkflowOrigin) -> PipelineWorkflowSources {
    switch origin {
    case .connected: sources.workflows
    case .recorded: sources.recordedWorkflows
    case .downloaded: sources.downloadedWorkflows
    }
  }

  private var canRun: Bool {
    !query.isEmpty && query.unicodeScalars.count <= 512 && !loadingCorpus && !restoringSources
      && (referenceCorpus == nil || readyReference != nil)
      && (selectedWorkflow.map { selected in
        workflowSources(workflowOrigin ?? .connected).isActive(selected)
      }
        ?? true)
  }

  private var inspectorSize: CGSize {
    let frame = NSScreen.main?.visibleFrame.size ?? CGSize(width: 1280, height: 900)
    return CGSize(width: min(1120, frame.width - 48), height: min(770, frame.height - 104))
  }

  var body: some View {
    VStack(spacing: 0) {
      header
      Divider()
      HStack(spacing: 0) {
        stages
        Divider()
        ZStack {
          if !benchmarkSelected { detail }
          benchmarkRunner
            .opacity(benchmarkSelected ? 1 : 0)
            .allowsHitTesting(benchmarkSelected)
            .accessibilityHidden(!benchmarkSelected)
        }
      }
    }
    .frame(width: inspectorSize.width, height: inspectorSize.height)
    .interactiveDismissDisabled(requestID != nil || loadingCorpus || benchmarkRunning)
    .sheet(isPresented: $showsSourceManager) { sourceManager }
    .sheet(isPresented: $showsFixtureCatalog) { fixtureCatalog }
    .sheet(isPresented: $showsDocumentEditor) {
      BenchmarkDocumentEditor { document in try appendDocuments([document]) }
    }
    .fileImporter(
      isPresented: $showsImporter,
      allowedContentTypes: importKind == .suite ? [.json] : [.data],
      allowsMultipleSelection: importKind == .files,
      onCompletion: importFiles
    )
    .task {
      composerFocused = true
      await restoreSources()
    }
  }

  private var benchmarkRunner: some View {
    let references =
      PipelineBenchmark.Reference.bundled + PipelineBenchmark.Catalog.bundled.references
      + (importedReference.map { [$0] } ?? [])
    let groups = Dictionary(grouping: references, by: \.source)
    var suites = groups.keys.sorted {
      ($0.repository, $0.revision) < ($1.repository, $1.revision)
    }.map { reference in
      var seen: Set<String> = []
      let questions = (groups[reference] ?? []).flatMap(\.questions).filter {
        seen.insert(
          $0.query.lowercased().split(whereSeparator: \.isWhitespace).joined(separator: " ")
        )
        .inserted
      }
      return BenchmarkRunnerView.Suite(
        id: "code-\(reference.repository)-\(reference.revision)",
        title: "\(reference.repository) · \(reference.revision.prefix(8))",
        ready: sources.loadedCorpora[reference] != nil,
        reference: reference,
        questions: questions.map {
          .init(query: $0.query, expected: $0.expected, action: nil, sourceURL: $0.source)
        })
    }
    if importedReference == nil, !customQuestions.isEmpty, !documents.isEmpty {
      suites.append(
        .init(
          id: "temporary", title: locale.localized("Temporary files"), ready: true, reference: nil,
          questions: customQuestions.map {
            .init(query: $0.query, expected: $0.expected, action: nil, sourceURL: $0.source)
          },
          documents: documents))
    }
    let recorded = PipelineBenchmark.Catalog.bundled.fixtureCases.filter { $0.action != nil }
    let recordedQuestions: [BenchmarkRunnerView.Question] = recorded.compactMap { item in
      guard let action = item.action else { return nil }
      let workflow = sources.recordedWorkflows.all.first {
        $0.repository == action.repository && $0.runID == action.runID
          && $0.attempt == action.attempt
      }
      return .init(
        query: item.query, expected: item.expected,
        action: .init(repository: action.repository, runId: action.runID, attempt: action.attempt),
        sourceLabel: workflow.map { "\($0.repository) · \($0.name) · \($0.branch)" }
          ?? action.repository,
        available: workflow.map(sources.recordedWorkflows.isActive) ?? false)
    }
    if !recordedQuestions.isEmpty {
      suites.append(
        .init(
          id: "recorded-actions", title: locale.localized("Recorded Benchmark Attempts"),
          ready: recordedQuestions.contains(where: \.available), reference: nil,
          questions: recordedQuestions))
    }
    let activeDownloads = Set(sources.downloadedWorkflows.active.map(\.id))
    let downloads = sources.actionDownloads.values.sorted {
      ($0.reference.repository, $0.reference.revision) < (
        $1.reference.repository, $1.reference.revision
      )
    }
    for download in downloads {
      let repo = download.reference.repository
      let questions = downloadedActionQuestions(download, active: activeDownloads)
      if !questions.isEmpty {
        suites.append(
          .init(
            id: "download-\(repo)-\(download.reference.revision)",
            title: "\(repo) · Actions · \(download.reference.revision.prefix(8))", ready: true,
            reference: nil, questions: questions))
      }
    }
    return BenchmarkRunnerView(
      suites: suites.filter { !$0.questions.isEmpty },
      preferredReference: referenceCorpus?.source, supervisor: supervisor,
      isSelected: benchmarkSelected, running: $benchmarkRunning,
      manageSources: { showsSourceManager = true },
      corpusSaved: { sources.savedCorpora[$0] != nil },
      activateCorpus: { source in
        if let reference = managedReferences.first(where: { $0.source == source }) {
          loadReference(reference)
        }
      },
      loadingCorpus: loadingCorpus || restoringSources, inspect: inspectBenchmarkResult)
  }

  private func downloadedActionQuestions(
    _ download: Components.Schemas.BenchmarkActionDownload, active: Set<String>
  ) -> [BenchmarkRunnerView.Question] {
    let prompts = PipelineBenchmark.Catalog.bundled.workflowQuestionTemplates ?? [:]
    return download.snapshots.flatMap { snapshot in
      let names = Dictionary(
        snapshot.workflows.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
      return snapshot.runs.flatMap { run -> [BenchmarkRunnerView.Question] in
        let id = "\(snapshot.repositoryId)/\(snapshot.branch)/\(run.id)/\(run.attempt)"
        guard active.contains(id) else { return [] }
        let action = Components.Schemas.BenchmarkAction(
          repository: snapshot.repository, runId: run.id, attempt: run.attempt, downloaded: true)
        let source = [
          names[run.workflowId] ?? snapshot.repository, snapshot.branch,
          run.createdAt.formatted(date: .abbreviated, time: .shortened),
        ].joined(separator: " · ")
        let fields = [
          ("event", run.event),
          (run.conclusion == nil ? "status" : "conclusion", run.conclusion ?? run.status),
          ("branch", run.headBranch),
        ]
        return fields.compactMap { field, expected in
          guard let query = prompts[field], let expected, !expected.isEmpty else { return nil }
          return .init(query: query, expected: expected, action: action, sourceLabel: source)
        }
      }
    }
  }

  private func inspectBenchmarkResult(
    _ suite: BenchmarkRunnerView.Suite, _ item: BenchmarkRunnerView.Question,
    _ result: AnswerResponse
  ) {
    let reference = suite.reference.flatMap { key in managedReferences.first { $0.source == key } }
    let action = item.action
    let actionOrigin: WorkflowOrigin = action?.downloaded == true ? .downloaded : .recorded
    let workflow = action.flatMap { target in
      workflowSources(actionOrigin).all.first {
        $0.repository == target.repository && $0.runID == target.runId
          && $0.attempt == target.attempt
      }
    }
    let prompt = PipelineBenchmark.Question(
      title: item.query, query: item.query, expected: item.expected, source: nil)
    select(prompt, reference: reference)
    if let workflow { scope = .workflow(workflow, origin: actionOrigin) }
    if suite.documents != nil { scope = .temporary }
    submittedQuestion = item.query
    submittedRepository = nil
    submittedCorpus = suite.reference.flatMap { sources.loadedCorpora[$0] }
    submittedWorkflow = workflow
    submittedBenchmarkBM25 = suite.reference != nil && result.answer.mode == .bm25
    submittedFiles = suite.documents?.map(\.path) ?? []
    submittedBytes = suite.documents?.reduce(0) { $0 + $1.text.utf8.count } ?? 0
    selectedCandidate = nil
    expandedRetrievalCandidate = nil
    focusedSlot = nil
    expandedModelSlots = []
    response = result
    stage = .retrieval
    benchmarkSelected = false
  }

  private var header: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack {
        Label("Inspect Pipeline", systemImage: "point.topleft.down.to.point.bottomright.curvepath")
          .font(.title2.weight(.semibold))
          .lineLimit(1)
          .labelStyle(PipelineHeadingStyle(tint: .indigo))
        Spacer()
        sourceControl
        Button("Done") { dismiss() }
          .keyboardShortcut(showsSources ? nil : .cancelAction)
          .disabled(requestID != nil || loadingCorpus || benchmarkRunning)
          .accessibilityIdentifier("answer-pipeline-close")
      }
      if !benchmarkSelected { queryComposer }
    }
    .padding(.horizontal, 22)
    .padding(.vertical, 14)
  }

  private var benchmarkModePicker: some View {
    Picker("Search mode", selection: $benchmarkRetrieval) {
      Text("Hybrid").tag(Components.Schemas.BenchmarkQuestion.retrievalPayload.hybrid)
      Text("BM25").tag(Components.Schemas.BenchmarkQuestion.retrievalPayload.bm25)
    }
    .pickerStyle(.menu)
    .fixedSize()
    .accessibilityIdentifier("benchmark-retrieval-mode")
  }

  private var queryComposer: some View {
    HStack(alignment: .center, spacing: 8) {
      TextField("Question about the local corpus", text: $question)
        .textFieldStyle(.plain)
        .focusEffectDisabled()
        .focused($composerFocused)
        .onSubmit(run)
        .padding(.horizontal, 9)
        .frame(height: 26)
        .background(.background, in: .rect(cornerRadius: 6))
        .overlay {
          RoundedRectangle(cornerRadius: 6)
            .strokeBorder(
              composerFocused ? Color.accentColor.opacity(0.65) : Color.secondary.opacity(0.25),
              lineWidth: 1
            )
            .allowsHitTesting(false)
        }
        .accessibilityIdentifier("answer-pipeline-question")
      questionMenu
      Button {
        if requestID != nil { cancel() } else { run() }
      } label: {
        Group {
          if requestID != nil {
            Label("Cancel", systemImage: "stop.fill")
          } else {
            HStack(spacing: 5) {
              Text("Run")
              Image(systemName: "return").accessibilityHidden(true)
            }
          }
        }
        .lineLimit(1)
        .frame(width: 90)
      }
      .buttonStyle(.borderedProminent)
      .disabled(requestID != nil ? cancelling : !canRun)
      .accessibilityIdentifier(requestID == nil ? "answer-pipeline-run" : "answer-pipeline-cancel")
    }
  }

  private var questionMenu: some View {
    Menu {
      if let reference = referenceCorpus, !reference.questions.isEmpty {
        Section("Questions for This Corpus") { questionButtons(reference) }
      }
      if !customQuestions.isEmpty {
        Menu("Imported Questions") {
          ForEach(Array(customQuestions.enumerated()), id: \.offset) { _, item in
            Button(item.title) { select(item, reference: importedReference) }
          }
        }
      }
      Menu("Quick Examples") {
        ForEach(PipelineBenchmark.Reference.bundled, id: \.source) { reference in
          Menu(reference.title) { questionButtons(reference) }
        }
      }
      let groups = Dictionary(grouping: PipelineBenchmark.Catalog.bundled.references) {
        $0.suite ?? ""
      }
      ForEach(groups.keys.sorted(), id: \.self) { title in
        Menu(title) {
          ForEach(groups[title] ?? [], id: \.source) { reference in
            Menu(reference.title) { questionButtons(reference) }
          }
        }
      }
      Divider()
      Button("GitHub Replay Questions…") { showsFixtureCatalog = true }
      Button("Import Question Set (JSON)…", systemImage: "square.and.arrow.down") {
        openImporter(.suite)
      }
      .accessibilityIdentifier("benchmark-import")
    } label: {
      Label("Questions", systemImage: "list.bullet")
    }
    .fixedSize()
    .disabled(requestID != nil || loadingCorpus || restoringSources)
    .accessibilityIdentifier("benchmark-questions-for-corpus")
  }

  private func questionButtons(_ reference: PipelineBenchmark.Reference) -> some View {
    ForEach(Array(reference.questions.enumerated()), id: \.offset) { _, question in
      Button(question.title) { select(question, reference: reference) }
        .accessibilityIdentifier("benchmark-case-\(question.title)")
    }
  }

  private var sourceScopeTitle: String {
    if benchmarkSelected { return locale.localized("Sources") }
    if let selectedWorkflow { return "\(selectedWorkflow.name) · #\(selectedWorkflow.runID)" }
    if let referenceCorpus {
      return "\(referenceCorpus.repository) · \(referenceCorpus.revision.prefix(8))"
    }
    if scope == .temporary { return locale.localized("Temporary corpus") }
    if let selectedRepository { return selectedRepository.repository.fullName }
    return locale.localized("All Connected Sources")
  }

  private var sourcesLoading: Bool { loadingCorpus || restoringSources }

  private var sourceWarning: String? { importFeedback ?? setupFeedback }

  private var sourceControl: some View {
    Button {
      showsSources.toggle()
    } label: {
      HStack(spacing: 7) {
        Image(systemName: "tray.full")
        Text(verbatim: sourceScopeTitle)
          .lineLimit(1)
          .truncationMode(.middle)
          .frame(maxWidth: .infinity, alignment: .leading)
        ZStack {
          if sourcesLoading {
            ProgressView().controlSize(.small)
          } else if sourceWarning != nil {
            Image(systemName: "exclamationmark.circle").foregroundStyle(.orange)
          } else if !benchmarkSelected, referenceCorpus != nil, readyReference == nil {
            Image(systemName: "arrow.down.circle").foregroundStyle(.secondary)
          }
        }
        .frame(width: 16, height: 16)
        Image(systemName: "chevron.down").font(.caption2.weight(.semibold))
          .foregroundStyle(.secondary)
      }
      .frame(width: 236)
    }
    .accessibilityLabel("Sources")
    .accessibilityValue(
      sourcesLoading
        ? locale.localized("Loading…")
        : sourceWarning.map { "\(sourceScopeTitle). \($0)" } ?? sourceScopeTitle
    )
    .accessibilityIdentifier("answer-pipeline-scope")
    .help(sourceWarning ?? locale.localized("Choose all connected sources or one specific source"))
    .disabled(requestID != nil || benchmarkRunning)
    .popover(isPresented: $showsSources) {
      sourcePopover
        .onExitCommand { showsSources = false }
    }
  }

  private var sourcePopover: some View {
    VStack(alignment: .leading, spacing: 10) {
      Text(benchmarkSelected ? "Sources" : "Search in")
        .font(.headline)
        .accessibilityAddTraits(.isHeader)
      if sourcesLoading {
        ProgressView("Loading…").controlSize(.small)
      }
      if let sourceWarning {
        Label(sourceWarning, systemImage: "exclamationmark.circle")
          .font(.caption)
          .foregroundStyle(.orange)
          .lineLimit(3)
          .help(sourceWarning)
          .accessibilityIdentifier("benchmark-import-feedback")
      }
      if !benchmarkSelected {
        if let reference = referenceCorpus {
          HStack {
            referenceStatus(reference)
            Spacer()
            benchmarkModePicker
          }
          .disabled(sourcesLoading)
        }
        Divider()
        ScrollView {
          VStack(alignment: .leading, spacing: 8) {
            sourceChoices
          }
          .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(height: 255)
        .disabled(sourcesLoading)
        Divider()
      }
      HStack {
        Spacer()
        if !benchmarkSelected { addCorpusMenu }
        Button("Manage Sources…", systemImage: "slider.horizontal.3") {
          showsSources = false
          showsSourceManager = true
        }
        .accessibilityIdentifier("pipeline-manage-sources")
      }
      .disabled(sourcesLoading)
    }
    .padding(14)
    .frame(width: 388)
  }

  @ViewBuilder private var sourceChoices: some View {
    sourceRow(.all, title: locale.localized("All Connected Sources"))
    if !documents.isEmpty {
      sourceRow(.temporary, title: locale.localized("Temporary files"))
    }
    sourceSection("Benchmark Corpora") {
      ForEach(managedReferences, id: \.source) { reference in
        sourceRow(
          .reference(reference.source),
          title: "\(reference.repository) · \(reference.revision.prefix(8))",
          downloadable: sources.savedCorpora[reference.source] == nil
        )
        .accessibilityIdentifier("benchmark-corpus-\(reference.repository)-\(reference.revision)")
      }
    }
    let currentRepositories = repositories.filter { $0.features.currentCode }
    if !currentRepositories.isEmpty {
      sourceSection("Repositories") {
        ForEach(currentRepositories, id: \.repository.id) { item in
          sourceRow(
            .repository(item.repository.id), title: "\(item.repository.fullName) · \(item.branch)")
        }
      }
    }
    if !sources.workflows.active.isEmpty {
      sourceSection("GitHub Actions") {
        ForEach(sources.workflows.active) { workflow in
          sourceRow(
            .workflow(workflow, origin: .connected),
            title: "\(workflow.repository) · \(workflow.name)",
            detail: locale.localized("Run \(workflow.runID) · Attempt \(workflow.attempt)")
          )
          .accessibilityIdentifier("pipeline-workflow-source-\(workflow.id)")
        }
      }
    }
    if !sources.downloadedWorkflows.active.isEmpty {
      sourceSection("Downloaded Benchmark Actions") {
        let groups = Dictionary(grouping: sources.downloadedWorkflows.active, by: \.repository)
        ForEach(groups.keys.sorted(), id: \.self) { repository in
          workflowScopeMenu(repository, workflows: groups[repository] ?? [], origin: .downloaded)
        }
      }
    }
    if !sources.recordedWorkflows.active.isEmpty {
      sourceSection("Recorded Benchmark Actions") {
        ForEach(sources.recordedSnapshots, id: \.repositoryId) { snapshot in
          let active = sources.recordedWorkflows.active.filter {
            $0.repositoryID == snapshot.repositoryId
          }
          if !active.isEmpty {
            workflowScopeMenu(snapshot.repository, workflows: active, origin: .recorded)
          }
        }
      }
    }
  }

  private func sourceSection<Content: View>(
    _ title: LocalizedStringKey, @ViewBuilder content: () -> Content
  ) -> some View {
    VStack(alignment: .leading, spacing: 2) {
      Text(title)
        .font(.caption.weight(.semibold))
        .foregroundStyle(.secondary)
        .padding(.leading, 7)
        .accessibilityAddTraits(.isHeader)
      content()
    }
  }

  private func sourceRow(
    _ next: CorpusScope, title: String, detail: String? = nil, downloadable: Bool = false
  ) -> some View {
    let selected = scope == next
    return Button {
      selectScope(next)
    } label: {
      HStack(spacing: 7) {
        ZStack {
          if selected {
            Image(systemName: "checkmark").foregroundStyle(Color.accentColor)
          } else if downloadable {
            Image(systemName: "arrow.down.circle").foregroundStyle(.secondary)
          }
        }
        .frame(width: 16)
        .accessibilityHidden(true)
        VStack(alignment: .leading, spacing: 2) {
          Text(verbatim: title).lineLimit(1).truncationMode(.middle)
          if let detail {
            Text(verbatim: detail).font(.caption.monospaced()).foregroundStyle(.secondary)
          }
        }
        Spacer(minLength: 0)
      }
      .padding(.horizontal, 7)
      .padding(.vertical, 4)
      .background(
        selected ? Color.accentColor.opacity(0.12) : .clear, in: .rect(cornerRadius: 5)
      )
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(selected ? [.isSelected] : [])
    .help(detail.map { "\(title)\n\($0)" } ?? title)
  }

  private func workflowScopeMenu(
    _ repository: String, workflows: [PipelineWorkflow], origin: WorkflowOrigin
  ) -> some View {
    Menu(repository) {
      ForEach(workflows) { workflow in
        Button {
          selectScope(.workflow(workflow, origin: origin))
        } label: {
          Text(verbatim: "\(workflow.name) · #\(workflow.runID) · \(workflow.commit.prefix(8))")
        }
      }
    }
    .padding(.leading, 7)
  }

  private func selectScope(_ next: CorpusScope) {
    guard scope != next else { return }
    scope = next
    response = nil
    feedback = nil
    hybridUnavailable = false
  }

  private var addCorpusMenu: some View {
    Menu {
      Button("Add Files…", systemImage: "doc.badge.plus") {
        showsSources = false
        openImporter(.files)
      }
      Button("Paste Text…", systemImage: "clipboard") {
        showsSources = false
        showsDocumentEditor = true
      }
      .accessibilityIdentifier("benchmark-add-text")
      if !documents.isEmpty {
        Divider()
        Button("Clear Temporary Corpus", role: .destructive) {
          documents = []
          if scope == .temporary { scope = .all }
        }
      }
    } label: {
      Label("Add", systemImage: "plus")
    }
    .fixedSize()
    .accessibilityIdentifier("answer-pipeline-import-corpus")
  }

  @ViewBuilder private func referenceStatus(_ reference: PipelineBenchmark.Reference) -> some View {
    if let loaded = readyReference {
      Label("Ready", systemImage: "checkmark.circle")
        .foregroundStyle(.green)
        .help("\(loaded.files) files indexed · \(reference.revision.prefix(12))")
        .accessibilityIdentifier("benchmark-reference-ready")
    } else {
      Button(sources.savedCorpora[reference.source] == nil ? "Download Corpus" : "Activate") {
        loadReference(reference)
      }
      .accessibilityIdentifier("benchmark-load-reference")
    }
  }

  private func loadReference(_ reference: PipelineBenchmark.Reference) {
    guard !loadingCorpus, requestID == nil else { return }
    loadingCorpus = true
    pendingReference = reference.source
    importFeedback = nil
    sourceManagerFeedback = nil
    Task {
      defer {
        loadingCorpus = false
        pendingReference = nil
      }
      do {
        let session = try await supervisor.connect(startIfNeeded: false)
        try await importReference(reference, session: session)
        if !showsSources { composerFocused = true }
      } catch {
        importFeedback = error.localizedDescription
      }
    }
  }

  private func importReference(_ reference: PipelineBenchmark.Reference, session: SessionClient)
    async throws
  {
    let corpus = try await session.loadBenchmarkCorpus(reference.source)
    sources.loadedCorpora[reference.source] = corpus
    sources.savedCorpora[reference.source] = corpus
  }

  private var sourceManager: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack {
        Text("Manage Sources").font(.title2.weight(.semibold))
        Spacer()
        Button("Done") { showsSourceManager = false }.keyboardShortcut(.cancelAction)
          .disabled(loadingCorpus)
          .accessibilityIdentifier("pipeline-manage-sources-done")
      }
      .padding(.horizontal, 18).padding(.vertical, 13)
      Divider()
      HStack(spacing: 0) {
        sourceManagerNavigation
        Divider()
        VStack(alignment: .leading, spacing: 0) {
          HStack(spacing: 12) {
            Text(sourceManagerTab.title).font(.headline).lineLimit(2)
              .accessibilityAddTraits(.isHeader)
            Spacer()
            sourceManagerAction
              .fixedSize()
          }
          .padding(.horizontal, 18)
          .frame(height: 52)
          .disabled(loadingCorpus)
          Divider()
          sourceManagerContent
        }
      }
      Divider()
      sourceManagerStatus
        .padding(.horizontal, 18).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: 44)
    }
    .frame(width: min(920, inspectorSize.width), height: min(620, inspectorSize.height))
    .interactiveDismissDisabled(loadingCorpus)
  }

  private var sourceManagerContent: some View {
    ZStack {
      if sourceManagerTab == .recordedActions && sources.recordedSnapshots.isEmpty {
        ContentUnavailableView("No Recorded Captures", systemImage: "archivebox")
          .opacity(loadingCorpus ? 0 : 1)
          .accessibilityHidden(loadingCorpus)
      } else if sourceManagerTab == .connectedActions && actionRepositories.isEmpty {
        ContentUnavailableView {
          Label("Connected Repositories", systemImage: "link")
        } description: {
          Text("Enable GitHub Actions for a connected repository to load its runs.")
        }
      } else {
        ScrollView {
          VStack(alignment: .leading, spacing: 12) {
            switch sourceManagerTab {
            case .corpora: benchmarkCorpusDownloads
            case .publicActions: benchmarkActionDownloads
            case .connectedActions: connectedActions
            case .recordedActions: recordedActions
            }
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(18)
          .disabled(loadingCorpus)
        }
      }
      if loadingCorpus
        && (sourceBatchStarted == nil && pendingReference == nil
          || sourceManagerTab == .recordedActions && sources.recordedSnapshots.isEmpty)
      {
        ProgressView().padding(16).background(.regularMaterial, in: .rect(cornerRadius: 8))
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }

  private var sourceManagerNavigation: some View {
    List {
      sourceManagerLink("Benchmark Corpora", icon: "tray.full", tab: .corpora)
      Section("GitHub Actions") {
        sourceManagerLink("Public Downloads", icon: "arrow.down.circle", tab: .publicActions)
        sourceManagerLink("Repositories", icon: "link", tab: .connectedActions)
        sourceManagerLink("Recorded Captures", icon: "archivebox", tab: .recordedActions)
      }
    }
    .listStyle(.sidebar)
    .frame(width: 205)
    .disabled(loadingCorpus)
    .accessibilityIdentifier("pipeline-source-manager-tab")
  }

  private func sourceManagerLink(
    _ title: LocalizedStringKey, icon: String, tab: SourceManagerTab
  ) -> some View {
    Button {
      sourceManagerTab = tab
    } label: {
      Label(title, systemImage: icon)
        .lineLimit(2)
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .listRowBackground(
      sourceManagerTab == tab ? Color.accentColor.opacity(0.18) : Color.clear
    )
    .accessibilityAddTraits(sourceManagerTab == tab ? .isSelected : [])
  }

  @ViewBuilder private var sourceManagerAction: some View {
    switch sourceManagerTab {
    case .corpora:
      let pending = managedReferences.filter { sources.loadedCorpora[$0.source] == nil }
      let needsDownload = pending.contains { sources.savedCorpora[$0.source] == nil }
      Button(needsDownload ? "Download & Activate All" : "Activate All") {
        importBatch(references: pending, actionRepositories: [])
      }
      .disabled(pending.isEmpty)
    case .publicActions:
      Button("Download All Actions") {
        importBatch(references: [], actionRepositories: [], benchmarkActions: managedReferences)
      }
      .disabled(managedReferences.isEmpty)
      .accessibilityIdentifier("pipeline-download-all-actions")
    case .connectedActions:
      Button("Refresh") {
        importBatch(references: [], actionRepositories: actionRepositories)
      }
      .disabled(actionRepositories.isEmpty)
      .accessibilityIdentifier("pipeline-load-workflow-runs")
    case .recordedActions:
      if sources.recordedSnapshots.isEmpty {
        Button("Load Recorded Captures") {
          importBatch(references: [], actionRepositories: [], loadCaptures: true)
        }
      }
    }
  }

  private var benchmarkCorpusDownloads: some View {
    PipelineGroup("Benchmark Corpora", tint: .indigo) {
      ForEach(managedReferences, id: \.source) { reference in
        if reference.source != managedReferences.first?.source { Divider() }
        benchmarkCorpusRow(reference)
      }
    }
  }

  @ViewBuilder private var sourceManagerStatus: some View {
    if let started = sourceBatchStarted {
      TimelineView(.periodic(from: started, by: 1)) { context in
        HStack {
          ProgressView().controlSize(.small)
          Text(verbatim: sourceBatchCurrent).lineLimit(1)
          Spacer()
          Text(verbatim: elapsed(from: started, to: context.date)).monospacedDigit()
        }
      }
    } else if let message = sourceManagerFeedback ?? importFeedback {
      Label(message, systemImage: "exclamationmark.circle")
        .foregroundStyle(.orange).lineLimit(1).help(message)
    } else if let sourceManagerReport {
      Text(verbatim: sourceManagerReport).foregroundStyle(.secondary)
    } else if sourceManagerTab == .corpora {
      Text("Downloaded corpora stay on this Mac when deactivated.").foregroundStyle(.secondary)
    } else {
      Text("Active attempts are available in Search in.").foregroundStyle(.secondary)
    }
  }

  private func corpusIdentity(_ reference: PipelineBenchmark.Reference) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      Text(verbatim: reference.repository).font(.body.weight(.medium))
      Text(verbatim: String(reference.revision.prefix(12)))
        .font(.caption.monospaced()).foregroundStyle(.secondary)
    }
    .padding(.vertical, 3)
  }

  private func referenceActivity(_ source: Components.Schemas.BenchmarkReference) -> some View {
    sourceActivity(pendingReference == source)
  }

  private func sourceActivity(_ pending: Bool) -> some View {
    Color.clear.frame(width: 18, height: 18)
      .overlay {
        if pending { ProgressView().controlSize(.small) }
      }
  }

  private func benchmarkCorpusRow(_ reference: PipelineBenchmark.Reference) -> some View {
    HStack {
      corpusIdentity(reference)
      Spacer()
      referenceActivity(reference.source)
      Group {
        if sources.savedCorpora[reference.source] == nil {
          Button("Download") { loadReference(reference) }
        } else {
          Toggle(
            reference.repository,
            isOn: Binding(
              get: { sources.loadedCorpora[reference.source] != nil },
              set: { enabled in
                if enabled { loadReference(reference) } else { unloadReference(reference) }
              }
            )
          )
          .toggleStyle(.switch)
          .labelsHidden()
          .accessibilityIdentifier(
            "pipeline-active-corpus-\(reference.repository)-\(reference.revision)")
        }
      }
      .frame(width: 116, alignment: .trailing)
    }
    .frame(minHeight: 38)
  }

  private var actionRepositories: [Repository] { repositories.filter { $0.features.actions } }

  private var connectedActions: some View {
    VStack(alignment: .leading, spacing: 6) {
      ForEach(actionRepositories, id: \.repository.id) { repository in
        let id = repository.repository.id
        if id != actionRepositories.first?.repository.id { Divider() }
        if let runs = sources.workflows.runs(for: id), !runs.isEmpty {
          DisclosureGroup {
            workflowActivationRows(runs, origin: .connected)
          } label: {
            actionRepositoryRow(repository.repository.fullName, count: runs.count)
          }
        } else {
          HStack {
            Text(verbatim: repository.repository.fullName).lineLimit(1)
            Spacer()
            sourceActivity(pendingRepository == id)
            Button("Refresh") {
              importBatch(references: [], actionRepositories: [repository])
            }
            .accessibilityIdentifier("pipeline-load-workflow-runs-\(id)")
            .frame(width: 104, alignment: .trailing)
          }
          .frame(minHeight: 32)
        }
      }
    }
  }

  private var benchmarkActionDownloads: some View {
    VStack(alignment: .leading, spacing: 6) {
      ForEach(managedReferences, id: \.source) { reference in
        if reference.source != managedReferences.first?.source { Divider() }
        let download = sources.actionDownloads[reference.source]
        let runs = (download?.snapshots ?? []).flatMap { snapshot in
          (sources.downloadedWorkflows.runs(for: snapshot.repositoryId) ?? []).filter {
            $0.commit == reference.revision && $0.branch == snapshot.branch
          }
        }
        HStack {
          corpusIdentity(reference)
          Spacer()
          referenceActivity(reference.source)
          actionRunCount(runs.count)
            .opacity(download == nil ? 0 : 1)
            .accessibilityHidden(download == nil)
            .frame(width: 96, alignment: .trailing)
          Button(download == nil ? "Download" : "Refresh") {
            importBatch(references: [], actionRepositories: [], benchmarkActions: [reference])
          }
          .accessibilityIdentifier(
            "pipeline-download-actions-\(reference.repository)-\(reference.revision)"
          )
          .frame(width: 104, alignment: .trailing)
        }
        if !runs.isEmpty {
          DisclosureGroup {
            workflowActivationRows(runs, origin: .downloaded)
          } label: {
            Text("Workflow Runs").frame(maxWidth: .infinity, alignment: .leading)
          }
        }
      }
    }
  }

  private var recordedActions: some View {
    VStack(alignment: .leading, spacing: 14) {
      recordedActionsGroup("Recorded Benchmark Attempts", pinned: true)
      recordedActionsGroup("Runs on other commits", pinned: false)
    }
  }

  private func recordedActionsGroup(_ title: LocalizedStringKey, pinned: Bool) -> some View {
    PipelineGroup(title, tint: .indigo) {
      ForEach(sources.recordedSnapshots, id: \.repositoryId) { snapshot in
        let runs = (sources.recordedWorkflows.runs(for: snapshot.repositoryId) ?? [])
          .filter { workflowMatchesBenchmark($0) == pinned }
        if !runs.isEmpty {
          DisclosureGroup {
            workflowActivationRows(runs, origin: .recorded)
          } label: {
            actionRepositoryRow(snapshot.repository, count: runs.count)
          }
        }
      }
    }
  }

  private func actionRepositoryRow(_ name: String, count: Int) -> some View {
    HStack(spacing: 8) {
      Text(verbatim: name)
        .lineLimit(1).truncationMode(.middle).help(name)
      Spacer(minLength: 8)
      actionRunCount(count)
    }
    .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
  }

  @ViewBuilder private func actionRunCount(_ count: Int) -> some View {
    Group {
      if count == 1 {
        Text("1 run")
      } else {
        Text("\(count) runs")
      }
    }
    .font(.callout).monospacedDigit().foregroundStyle(.secondary).fixedSize()
  }

  private func workflowMatchesBenchmark(_ workflow: PipelineWorkflow) -> Bool {
    (PipelineBenchmark.Reference.bundled + PipelineBenchmark.Catalog.bundled.references)
      .contains { $0.repository == workflow.repository && $0.revision == workflow.commit }
  }

  private func workflowActivationRows(_ runs: [PipelineWorkflow], origin: WorkflowOrigin)
    -> some View
  {
    VStack(alignment: .leading, spacing: 0) {
      ForEach(runs) { workflow in
        if workflow.id != runs.first?.id { Divider() }
        workflowActivationRow(workflow, origin: origin)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.leading, 16)
  }

  private func workflowActivationRow(_ workflow: PipelineWorkflow, origin: WorkflowOrigin)
    -> some View
  {
    HStack(alignment: .center, spacing: 12) {
      workflowLabel(workflow, recorded: origin != .connected, showRepository: false)
        .frame(maxWidth: .infinity, alignment: .leading)
      Toggle(
        workflow.name,
        isOn: Binding(
          get: { workflowSources(origin).isActive(workflow) },
          set: { enabled in
            switch origin {
            case .recorded:
              sources.recordedWorkflows.setActive(enabled, for: workflow)
              sourcePreferences.recordedWorkflows = Set(sources.recordedWorkflows.active.map(\.id))
            case .downloaded:
              sources.downloadedWorkflows.setActive(enabled, for: workflow)
              sourcePreferences.downloadedWorkflows = Set(
                sources.downloadedWorkflows.active.map(\.id))
            case .connected:
              sources.workflows.setActive(enabled, for: workflow)
              if enabled {
                sourcePreferences.workflows.insert(workflow.id)
              } else {
                sourcePreferences.workflows.remove(workflow.id)
              }
            }
            saveSourcePreferences()
          }
        )
      )
      .labelsHidden()
      .toggleStyle(.switch)
      .accessibilityLabel(Text(verbatim: "\(workflow.repository) · \(workflow.name)"))
      .accessibilityIdentifier("pipeline-active-workflow-\(workflow.id)")
    }
    .padding(.vertical, 8)
  }

  private var managedReferences: [PipelineBenchmark.Reference] {
    var result = PipelineBenchmark.Reference.bundled
    let saved = sources.savedCorpora.values.sorted {
      ($0.repository, $0.revision) < ($1.repository, $1.revision)
    }.map {
      PipelineBenchmark.Reference(
        title: $0.repository, repository: $0.repository, revision: $0.revision, suite: nil,
        questions: [])
    }
    for reference in (importedReference.map { [$0] } ?? [])
      + PipelineBenchmark.Catalog.bundled.references + saved
    {
      if !result.contains(where: { $0.source == reference.source }) { result.append(reference) }
    }
    return result
  }

  private var fixtureCatalog: some View {
    let groups = Dictionary(grouping: PipelineBenchmark.Catalog.bundled.fixtureCases, by: \.suite)
    return VStack(alignment: .leading, spacing: 0) {
      HStack {
        Text("GitHub Benchmark Catalog").font(.title2.weight(.semibold))
        Spacer()
        Button("Done") { showsFixtureCatalog = false }.keyboardShortcut(.cancelAction)
      }
      .padding(16)
      Divider()
      ScrollView {
        VStack(alignment: .leading, spacing: 12) {
          Text("These cases use the recorded GitHub fixtures for their full replay.")
            .foregroundStyle(.secondary)
          ForEach(groups.keys.sorted(), id: \.self) { title in
            Text(verbatim: title).font(.headline)
            ForEach(groups[title] ?? []) { item in
              DisclosureGroup {
                VStack(alignment: .leading, spacing: 7) {
                  if let expected = item.expected { field("Expected answer", expected) }
                  Text(verbatim: item.oracle).font(.callout.monospaced()).textSelection(.enabled)
                  if let target = item.action {
                    if let workflow = sources.recordedWorkflows.active.first(where: {
                      $0.repository == target.repository && $0.runID == target.runID
                        && $0.attempt == target.attempt
                    }) {
                      Button("Use in Inspector") {
                        question = item.query
                        example = nil
                        selectScope(.workflow(workflow, origin: .recorded))
                        showsFixtureCatalog = false
                      }
                    } else {
                      Text("Activate the recorded workflow attempt in Manage Sources.")
                        .foregroundStyle(.secondary)
                    }
                  }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
              } label: {
                Text(verbatim: item.query)
              }
              Divider()
            }
          }
        }
        .padding(16)
      }
    }
    .frame(width: 700, height: 550)
  }

  private func workflowLabel(
    _ workflow: PipelineWorkflow, recorded: Bool = false, showRepository: Bool = true
  ) -> some View {
    VStack(alignment: .leading, spacing: 3) {
      Text(verbatim: showRepository ? "\(workflow.repository) · \(workflow.name)" : workflow.name)
      Text("Run \(workflow.runID) · Attempt \(workflow.attempt)")
        .font(.caption.monospaced()).foregroundStyle(.secondary)
      if recorded {
        HStack(spacing: 5) {
          if workflowMatchesBenchmark(workflow) {
            Text("Pinned commit")
          } else {
            Text("Other commit")
          }
          Text(verbatim: String(workflow.commit.prefix(8))).monospaced()
        }
        .font(.caption).foregroundStyle(.secondary)
      }
    }
  }

  private func unloadReference(_ reference: PipelineBenchmark.Reference) {
    guard !loadingCorpus, requestID == nil else { return }
    loadingCorpus = true
    pendingReference = reference.source
    importFeedback = nil
    sourceManagerFeedback = nil
    Task {
      defer {
        loadingCorpus = false
        pendingReference = nil
      }
      do {
        let session = try await supervisor.connect(startIfNeeded: false)
        try await session.unloadBenchmarkCorpus(reference.source)
        sources.loadedCorpora.removeValue(forKey: reference.source)

      } catch {
        sourceManagerFeedback = error.localizedDescription
      }
    }
  }

  private func importBatch(
    references: [PipelineBenchmark.Reference], actionRepositories: [Repository],
    benchmarkActions: [PipelineBenchmark.Reference] = [], loadCaptures: Bool = false
  ) {
    guard !loadingCorpus, requestID == nil else { return }
    loadingCorpus = true
    importFeedback = nil
    sourceManagerFeedback = nil
    sourceManagerReport = nil
    let started = Date.now
    sourceBatchStarted = started
    sourceBatchCurrent = locale.localized("Connecting…")
    Task {
      defer {
        sourceBatchStarted = nil
        pendingReference = nil
        pendingRepository = nil
        loadingCorpus = false
      }
      do {
        let session = try await supervisor.connect(startIfNeeded: false)
        await importSources(
          references: references, actionRepositories: actionRepositories,
          benchmarkActions: benchmarkActions, loadCaptures: loadCaptures, started: started,
          session: session)
      } catch {
        sourceManagerFeedback = error.localizedDescription
      }
    }
  }

  private func importSources(
    references: [PipelineBenchmark.Reference], actionRepositories: [Repository],
    benchmarkActions: [PipelineBenchmark.Reference], loadCaptures: Bool, started: Date,
    session: SessionClient
  ) async {
    var failures: [String] = []
    var succeeded = 0
    let total =
      references.count + actionRepositories.count + benchmarkActions.count
      + (loadCaptures ? 1 : 0)
    let corpusFailures = await importReferences(references, total: total, session: session)
    let actionFailures = await importReferences(
      benchmarkActions, total: total, session: session, downloadActions: true)
    failures.append(contentsOf: corpusFailures + actionFailures)
    succeeded +=
      references.count + benchmarkActions.count - corpusFailures.count
      - actionFailures.count
    pendingReference = nil
    if loadCaptures {
      do {
        try await loadRecordedActions(session)
        sourcePreferences.loadedCaptures = true
        saveSourcePreferences()
        succeeded += 1
      } catch {
        failures.append(error.localizedDescription)
      }
    }
    for (offset, repository) in actionRepositories.enumerated() {
      pendingRepository = repository.repository.id
      let name = repository.repository.fullName
      sourceBatchCurrent = "\(offset + 1)/\(total) · \(name)"
      do {
        if try await importConnectedActions(repository, session: session) {
          succeeded += 1
        } else {
          failures.append(
            "\(name) · \(locale.localized("No workflow runs have been collected."))")
        }
      } catch {
        failures.append("\(name) · \(error.localizedDescription)")
      }
    }
    sourceManagerReport = locale.localized(
      "Imported \(succeeded) of \(total) sources · \(elapsed(from: started, to: .now))")
    if !failures.isEmpty { sourceManagerFeedback = failures.joined(separator: "\n") }
  }

  private func importReferences(
    _ references: [PipelineBenchmark.Reference], total: Int, session: SessionClient,
    downloadActions: Bool = false
  ) async -> [String] {
    var failures: [String] = []
    for (offset, reference) in references.enumerated() {
      pendingReference = reference.source
      let name = downloadActions ? reference.repository : reference.title
      sourceBatchCurrent = "\(offset + 1)/\(total) · \(name)"
      do {
        if downloadActions {
          let previous = Set(sources.downloadedWorkflows.all.map(\.id))
          sources.actionDownloads[reference.source] = try await session.downloadBenchmarkActions(
            reference.source)
          rebuildDownloadedWorkflows(activatingNewSince: previous)
        } else {
          try await importReference(reference, session: session)
        }
      } catch {
        failures.append("\(reference.repository) · \(error.localizedDescription)")
      }
    }
    return failures
  }

  private func importConnectedActions(_ repository: Repository, session: SessionClient) async throws
    -> Bool
  {
    let id = repository.repository.id
    let state = try await session.gitHubActions(id: id)
    let snapshot: Components.Schemas.GitHubActions?
    if let current = state.snapshot, current.branch == repository.branch, !current.runs.isEmpty {
      snapshot = current
    } else {
      snapshot = try await session.refreshGitHubActions(id: id).snapshot
    }
    guard let snapshot, snapshot.branch == repository.branch, !snapshot.runs.isEmpty else {
      return false
    }
    sources.workflows.load(snapshot)
    sources.workflows.restoreSelection(sourcePreferences.workflows)
    return true
  }

  private func elapsed(from start: Date, to end: Date) -> String {
    Duration.seconds(max(0, end.timeIntervalSince(start)))
      .formatted(.units(allowed: [.minutes, .seconds], width: .abbreviated))
  }

  @ViewBuilder
  private func benchmarkReference(_ example: SelectedExample) -> some View {
    VStack(alignment: .leading, spacing: 3) {
      if let reference = example.reference, referenceCorpus?.source != reference.source {
        let revision = String(reference.revision.prefix(12))
        if selectedRepository?.repository.fullName == reference.repository,
          selectedRepository?.codeCollection?.commit == reference.revision
        {
          Label("Reference commit \(revision)", systemImage: "checkmark.circle")
            .foregroundStyle(.secondary)
        } else {
          Label(
            "Reference: \(reference.repository) · \(revision). This run uses the selected corpus.",
            systemImage: "info.circle"
          ).foregroundStyle(.secondary)
        }
      }
      if let expected = example.question.expected {
        DisclosureGroup("Expected answer") {
          Text(verbatim: expected).textSelection(.enabled)
          if let source = example.question.source, source.scheme == "https" {
            Link("Reference source", destination: source)
          }
        }
      }
    }
    .font(.caption)
    .accessibilityIdentifier("answer-pipeline-benchmark-reference")
  }

  private func select(_ item: PipelineBenchmark.Question, reference: PipelineBenchmark.Reference?) {
    example = .init(question: item, reference: reference)
    question = item.query
    response = nil
    feedback = nil
    importFeedback = nil
    if let reference {
      scope = .reference(reference.source)
    } else if case .reference = scope {
      scope = .all
    }
    composerFocused = true
  }

  private func openImporter(_ kind: BenchmarkImport) {
    importKind = kind
    showsImporter = true
  }

  private func importFiles(_ result: Result<[URL], Error>) {
    let kind = importKind
    do {
      let urls = try result.get()
      if kind == .suite, let url = urls.first {
        let suite = try PipelineBenchmark.load(url)
        importedReference = suite.reference.map {
          .init(
            title: $0.repository, repository: $0.repository,
            revision: $0.revision, suite: nil, questions: suite.questions)
        }
        if let files = suite.documents {
          documents = files
          scope = .temporary
        }
        customQuestions = suite.questions
        if let importedReference {
          sourcePreferences.importedQuestions = importedReference
          saveSourcePreferences()
        }
        if let first = suite.questions.first { select(first, reference: importedReference) }
      } else if !urls.isEmpty {
        try appendDocuments(PipelineBenchmark.files(urls))
      }
      importFeedback = nil
    } catch {
      importFeedback = error.localizedDescription
    }
  }

  private func appendDocuments(_ added: [PipelineBenchmark.Document]) throws {
    let combined = documents + added
    try PipelineBenchmark.validate(combined)
    documents = combined
    scope = .temporary
    importFeedback = nil
  }

  private var stages: some View {
    List {
      Button {
        benchmarkSelected = true
      } label: {
        HStack(spacing: 9) {
          Image(systemName: "play.rectangle").foregroundStyle(.indigo).frame(width: 24)
          Text("Benchmarks").font(.callout.weight(.medium))
          Spacer(minLength: 0)
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .disabled(restoringSources || loadingCorpus || requestID != nil)
      .listRowBackground(
        RoundedRectangle(cornerRadius: 7)
          .fill(benchmarkSelected ? Color.indigo.opacity(0.15) : .clear)
          .padding(.horizontal, 4)
      )
      .accessibilityAddTraits(benchmarkSelected ? [.isSelected] : [])
      .accessibilityIdentifier("answer-pipeline-benchmarks")
      Section("Sources") { stageRows([.corpus, .index]) }
      Section("Search") { stageRows([.retrieval, .embedding, .ranking]) }
      Section("Answer") { stageRows([.context, .generation, .verification]) }
    }
    .listStyle(.sidebar)
    .frame(width: inspectorSize.width < 950 ? 190 : 230)
  }

  private func stageRows(_ items: [Stage]) -> some View {
    ForEach(items, id: \.self) { item in
      Button {
        stage = item
        benchmarkSelected = false
      } label: {
        HStack(spacing: 9) {
          Image(systemName: item.icon)
            .foregroundStyle(item.tint)
            .frame(width: 24)
          VStack(alignment: .leading, spacing: 2) {
            Text(item.title).font(.callout.weight(.medium))
            stageSummary(item).font(.caption).foregroundStyle(.secondary)
              .lineLimit(1).frame(height: 14, alignment: .leading)
          }
          Spacer(minLength: 0)
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .disabled(benchmarkRunning)
      .listRowBackground(
        RoundedRectangle(cornerRadius: 7)
          .fill(!benchmarkSelected && stage == item ? item.tint.opacity(0.15) : Color.clear)
          .padding(.horizontal, 4)
      )
      .accessibilityAddTraits(!benchmarkSelected && stage == item ? [.isSelected] : [])
      .accessibilityIdentifier("answer-pipeline-stage-\(item.rawValue)")
    }
  }

  @ViewBuilder private func stageSummary(_ item: Stage) -> some View {
    if requestID != nil {
      Text("Running…")
    } else if let answer {
      switch item {
      case .corpus:
        corpusSummary
      case .index: Text("SQLite FTS5")
      case .retrieval: Text(answer.mode.rawValue.uppercased())
      case .embedding:
        embeddingSummary(answer)
      case .ranking:
        let count = review?.candidates.count ?? 0
        Text(count == 1 ? "1 candidate" : "\(count) candidates")
      case .context:
        let count = answer.modelContext?.sources.count ?? 0
        Text(count == 1 ? "1 passage" : "\(count) passages")
      case .generation:
        Text(capture?.draft == nil ? "No draft captured" : "Draft available")
      case .verification:
        Text(answer.outcome == .answered ? "Published" : "Insufficient evidence")
      }
    }
  }

  @ViewBuilder private var corpusSummary: some View {
    if let submittedWorkflow {
      Text(verbatim: submittedWorkflow.repository)
    } else if let submittedCorpus {
      Text(verbatim: submittedCorpus.repository)
    } else if submittedFiles.isEmpty {
      Text(submittedRepository?.repository.fullName ?? locale.localized("All sources"))
    } else {
      Text("Temporary corpus")
    }
  }

  @ViewBuilder private func embeddingSummary(_ answer: Answer) -> some View {
    if let embedding = capture?.embedding {
      Text("\(embedding.vector.count) dimensions")
    } else if answer.mode == .bm25 {
      Text("BM25 only")
    } else {
      Text("Unavailable")
    }
  }

  @ViewBuilder private var detail: some View {
    if restoringSources {
      ProgressView("Loading sources…")
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    } else if requestID != nil {
      Group {
        if indexingBenchmark {
          if let progress = indexProgress, progress.total > 0 {
            ProgressView(value: Double(progress.indexed), total: Double(progress.total)) {
              Text("Indexing benchmark embeddings")
            } currentValueLabel: {
              Text("\(progress.indexed) of \(progress.total) passages")
            }
            .frame(maxWidth: 320)
          } else {
            ProgressView("Preparing benchmark embeddings…")
          }
        } else {
          ProgressView("Running local Answers…")
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    } else if let feedback {
      ContentUnavailableView {
        Label("Request interrupted", systemImage: "exclamationmark.circle")
      } description: {
        Text(verbatim: feedback).textSelection(.enabled)
        if hybridUnavailable {
          Text("Hybrid requires a working embedding model. Check Models in Settings.")
        }
      } actions: {
        if hybridUnavailable && benchmarkRetrieval == .hybrid {
          Button("Use BM25") { benchmarkRetrieval = .bm25 }
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    } else if stage == .ranking, answer != nil {
      VStack(alignment: .leading, spacing: 12) {
        stageHeading
        ranking
      }
      .padding(18)
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    } else if answer != nil || stage == .corpus || stage == .index {
      ScrollViewReader { proxy in
        ScrollView {
          VStack(alignment: .leading, spacing: 16) {
            stageHeading
            stageContent
          }
          .padding(22)
          .frame(maxWidth: .infinity, alignment: .leading)
        }
        .task(id: focusedSlot) {
          if stage == .context, let focusedSlot { proxy.scrollTo(focusedSlot, anchor: .top) }
        }
      }
      .id(stage)
    } else {
      ContentUnavailableView(
        "Run a question", systemImage: "scope",
        description: Text("Inspect the exact local request from retrieval through verification.")
      )
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
  }

  private var stageHeading: some View {
    Label(stage.title, systemImage: stage.icon)
      .font(.title3.weight(.semibold))
      .labelStyle(PipelineHeadingStyle(tint: stage.tint))
      .accessibilityAddTraits(.isHeader)
  }

  @ViewBuilder private var stageContent: some View {
    switch stage {
    case .corpus: corpus
    case .index: index
    case .ranking: EmptyView()
    case .embedding: embeddingDetails
    case .retrieval:
      if let answer { retrieval(answer) }
    case .context:
      if let answer { context(answer) }
    case .generation:
      if let answer { generation(answer) }
    case .verification:
      if let answer { verification(answer) }
    }
  }

  private var corpus: some View {
    let loaded = answer == nil ? readyReference : submittedCorpus
    let files = answer == nil ? (scope == .temporary ? documents.map(\.path) : []) : submittedFiles
    let bytes = answer == nil ? documents.reduce(0) { $0 + $1.text.utf8.count } : submittedBytes
    let repository = answer == nil ? selectedRepository : submittedRepository
    let workflow = answer == nil ? selectedWorkflow : submittedWorkflow
    return VStack(alignment: .leading, spacing: 16) {
      PipelineGroup("Source Scope", tint: .indigo) {
        if let workflow {
          LabeledContent("Repository", value: workflow.repository)
          LabeledContent("Commit", value: workflow.commit).textSelection(.enabled)
          workflowLabel(workflow, recorded: workflowOrigin != .connected)
        } else if let loaded {
          LabeledContent("Repository", value: loaded.repository)
          LabeledContent("Commit", value: loaded.revision).textSelection(.enabled)
          LabeledContent("Indexed files", value: loaded.files.formatted())
            .accessibilityIdentifier("benchmark-loaded-file-count")
          LabeledContent("Source text", value: formattedBytes(loaded.bytes))
          LabeledContent("Storage") { Text("SQLite on this Mac") }
        } else if let referenceCorpus, answer == nil {
          LabeledContent("Repository", value: referenceCorpus.repository)
          LabeledContent("Commit", value: referenceCorpus.revision).textSelection(.enabled)
        } else if !files.isEmpty {
          LabeledContent("Scope", value: "Temporary corpus")
          LabeledContent("Source text", value: formattedBytes(bytes))
          LabeledContent("Storage") { Text("In-memory SQLite") }
        } else if let repository {
          LabeledContent("Repository", value: repository.repository.fullName)
          LabeledContent("Tracked branch", value: repository.branch)
          if let publication = repository.codeCollection {
            if let commit = publication.commit {
              LabeledContent("Published commit when loaded", value: commit)
            }
            if let files = publication.publishedFiles {
              LabeledContent("Published files when loaded", value: files.formatted())
            }
          }
        } else {
          LabeledContent("Scope", value: "All sources")
        }
      }
      if let selectedExample { benchmarkReference(selectedExample) }
      if let traceID = response?.traceID {
        PipelineGroup("Trace ID", tint: .indigo) {
          Text(verbatim: traceID).font(.caption.monospaced()).textSelection(.enabled)
        }
      }
      if !files.isEmpty {
        PipelineGroup("Imported Files", tint: .indigo) {
          ForEach(files, id: \.self) { path in
            Text(verbatim: path).font(.callout.monospaced()).textSelection(.enabled)
          }
        }
      }
    }
  }

  private var index: some View {
    let lexicalOnly =
      answer == nil
      ? scope == .temporary || (referenceCorpus != nil && benchmarkRetrieval == .bm25)
      : !submittedFiles.isEmpty || submittedBenchmarkBM25
    return PipelineGroup("Index Layout", tint: .indigo) {
      LabeledContent("Passages", value: "Local SQLite · documents")
      LabeledContent("Keyword index", value: "SQLite FTS5")
      LabeledContent("Chunk boundaries") { Text("Consecutive lines · up to 2,048 bytes") }
      if lexicalOnly {
        LabeledContent("Search mode", value: "BM25")
      } else {
        LabeledContent("Embedding storage", value: "Local SQLite · document_vectors")
      }
      Text(
        "Open a candidate in Ranking or a passage in Model Input to read its source and line range."
      )
      .foregroundStyle(.secondary)
    }
  }

  private func formattedBytes<T: BinaryInteger>(_ bytes: T) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
  }

  @ViewBuilder private func retrieval(_ answer: Answer) -> some View {
    PipelineGroup("Request", tint: .blue) {
      field("Question", submittedQuestion)
      LabeledContent("Search mode", value: answer.mode.rawValue.uppercased())
      if let fallback = capture?.fallback {
        LabeledContent("BM25 fallback", value: fallback)
      }
    }
    if let review {
      PipelineGroup("Sources searched", tint: .blue) {
        ForEach(Array(review.searches.enumerated()), id: \.offset) { _, search in
          HStack(alignment: .center) {
            VStack(alignment: .leading, spacing: 2) {
              switch search.source {
              case .localPassages: Text("Local source passages")
              case .githubHistory: Text("GitHub history")
              case .githubActions: Text("GitHub Actions")
              }
              if let selection = search.selection {
                Text(verbatim: selection).font(.caption).foregroundStyle(.secondary)
              }
            }
            .lineLimit(1)
            Spacer()
            Text("\(search.candidateCount) candidates")
              .monospacedDigit().foregroundStyle(.secondary)
          }
          .frame(height: 39)
        }
      }
      retrievedCandidates(review)
    }
  }

  @ViewBuilder private func retrievedCandidates(_ review: Components.Schemas.AnswerEvidenceReview)
    -> some View
  {
    let candidates = review.candidates.enumerated().filter { $0.element.contextAdded != true }
    let expectedCount = review.searches.reduce(0) { $0 + $1.candidateCount }
    if expectedCount > 0 || !candidates.isEmpty {
      PipelineGroup("Retrieved Candidates", tint: .blue) {
        if candidates.count < expectedCount {
          Text("Captured \(candidates.count) of \(expectedCount) candidates")
            .font(.caption).foregroundStyle(.secondary)
        }
        LazyVStack(alignment: .leading, spacing: 2) {
          ForEach(candidates, id: \.offset) { index, candidate in
            let expanded = expandedRetrievalCandidate == index
            Button {
              expandedRetrievalCandidate = expanded ? nil : index
            } label: {
              HStack(spacing: 3) {
                Image(systemName: expanded ? "chevron.down" : "chevron.right")
                  .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                  .frame(width: 14)
                candidateRow(candidate, selected: expanded)
              }
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(Text(expanded ? "Expanded" : "Collapsed"))
            .accessibilityIdentifier("pipeline-retrieval-candidate-\(index)")
            if expanded {
              Button("View in Ranking") {
                selectedCandidate = index
                stage = .ranking
              }
              .frame(maxWidth: .infinity, alignment: .trailing)
              .accessibilityIdentifier("pipeline-retrieval-view-ranking-\(index)")
              candidateDetail(candidate)
            }
          }
        }
      }
    }
  }

  @ViewBuilder private var embeddingDetails: some View {
    if let embedding = capture?.embedding {
      PipelineGroup("Query Embedding", tint: .blue) {
        LabeledContent("Embedding profile", value: embedding.profile)
        LabeledContent("Indexed passages", value: "\(embedding.indexed)/\(embedding.total)")
        embeddingInput(embedding.input, vector: embedding.vector)
        neighbors(embedding.nearest)
      }
      if let alternate = embedding.reformulation {
        PipelineGroup("Reformulated query", tint: .blue) {
          embeddingInput(alternate.input, vector: alternate.vector)
          neighbors(alternate.nearest)
        }
      }
    } else if let fallback = capture?.fallback {
      PipelineGroup("BM25 fallback", tint: .blue) {
        Text(verbatim: fallback).foregroundStyle(.secondary)
        Button("View Retrieval") { stage = .retrieval }
      }
    } else if submittedBenchmarkBM25 || !submittedFiles.isEmpty {
      PipelineGroup("BM25 only", tint: .blue) {
        Text("This run used keyword search. BM25 does not calculate a query embedding.")
          .foregroundStyle(.secondary)
        Button("View Retrieval") { stage = .retrieval }
      }
    } else if submittedWorkflow != nil {
      PipelineGroup("Exact workflow attempt", tint: .blue) {
        Text("The run and attempt identify the recorded evidence directly.")
          .foregroundStyle(.secondary)
      }
    } else {
      Text("No query embedding was captured for this request.").foregroundStyle(.secondary)
    }
  }

  private func embeddingInput<T: BinaryFloatingPoint>(_ input: String, vector: [T]) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      field("Embedding input", input)
      DisclosureGroup("Query vector · \(vector.count) dimensions") {
        excerpt(vector.map { String(describing: $0) }.joined(separator: ", "))
      }
    }
  }

  @ViewBuilder
  private func neighbors(_ matches: [Components.Schemas.AnswerVectorMatch]) -> some View {
    if !matches.isEmpty {
      HStack {
        Text("Nearest passages").font(.headline)
        Spacer()
        Text("Similarity").font(.caption).foregroundStyle(.secondary)
      }
      .accessibilityAddTraits(.isHeader)
      Text("Similarity is the dot product of the normalized query and passage vectors.")
        .font(.caption).foregroundStyle(.secondary)
      VStack(alignment: .leading, spacing: 0) {
        ForEach(Array(matches.enumerated()), id: \.offset) { index, match in
          if index > 0 { Divider() }
          neighborRow(match, rank: index + 1)
        }
      }
    }
  }

  private func neighborRow(_ match: Components.Schemas.AnswerVectorMatch, rank: Int) -> some View {
    let location = match.evidence.location
    let path = location?.path ?? match.evidence.title
    return DisclosureGroup {
      VStack(alignment: .leading, spacing: 8) {
        if let location {
          Text("Lines \(location.startLine)–\(location.endLine) · \(match.evidence.revision)")
            .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
          if let url = URL(string: location.fileUrl), url.host != "benchmark.invalid" {
            Link("Open source", destination: url)
          }
        }
        excerpt(match.text)
        DisclosureGroup("Stored vector · \(match.vector.count) dimensions") {
          excerpt(match.vector.map { String(describing: $0) }.joined(separator: ", "))
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.bottom, 8)
    } label: {
      HStack(alignment: .firstTextBaseline, spacing: 8) {
        Text("#\(rank)")
          .font(.callout.monospacedDigit()).foregroundStyle(.secondary)
          .frame(width: 32, alignment: .trailing)
        VStack(alignment: .leading, spacing: 2) {
          Text(verbatim: path).lineLimit(1).truncationMode(.middle).help(path)
          if let location {
            Text("Lines \(location.startLine)–\(location.endLine)")
              .font(.caption.monospacedDigit()).foregroundStyle(.secondary)
          }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        Text(match.similarity.formatted(.number.precision(.fractionLength(5))))
          .font(.callout.monospacedDigit())
          .frame(width: 68, alignment: .trailing)
      }
      .frame(maxWidth: .infinity, minHeight: 38, alignment: .leading)
    }
  }

  @ViewBuilder private var ranking: some View {
    if let candidates = review?.candidates, !candidates.isEmpty {
      let selected = selectedCandidate ?? candidates.startIndex
      let added = candidates.count { $0.contextAdded == true }
      if added > 0 {
        Text("\(candidates.count - added) retrieved · \(added) added to context")
          .font(.callout).foregroundStyle(.secondary)
          .accessibilityIdentifier("pipeline-ranking-provenance")
      }
      HStack(spacing: 12) {
        ScrollView {
          LazyVStack(alignment: .leading, spacing: 3) {
            ForEach(Array(candidates.enumerated()), id: \.offset) { index, candidate in
              Button {
                selectedCandidate = index
              } label: {
                candidateRow(candidate, selected: index == selected)
              }
              .buttonStyle(.plain)
              .accessibilityAddTraits(index == selected ? [.isSelected] : [])
              .accessibilityIdentifier("pipeline-ranking-candidate-\(index)")
            }
          }
          .padding(5)
        }
        .frame(minWidth: 260, idealWidth: 325, maxWidth: 365)
        .background(.quaternary.opacity(0.32), in: .rect(cornerRadius: 8))

        ScrollView {
          candidateDetail(candidates[selected])
        }
        .id(selected)
        .frame(maxWidth: .infinity, alignment: .leading)
      }
      .frame(maxHeight: .infinity)
    } else {
      ContentUnavailableView("No ranked candidates were captured.", systemImage: "list.number")
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
  }

  private func candidateRow(_ candidate: Candidate, selected: Bool) -> some View {
    HStack(alignment: .center, spacing: 8) {
      Group {
        if let rank = candidate.rank {
          Text("#\(rank)")
        } else if candidate.contextAdded == true {
          Image(systemName: "plus.circle").accessibilityLabel("Added to context")
        } else {
          Text(verbatim: "–").accessibilityLabel("Unranked")
        }
      }
      .font(.caption.monospacedDigit())
      .foregroundStyle(.secondary)
      .frame(width: 32, alignment: .trailing)
      VStack(alignment: .leading, spacing: 3) {
        Text(verbatim: fileName(candidate.location?.path ?? candidate.reference))
          .font(.callout.weight(.medium))
          .lineLimit(1)
          .truncationMode(.middle)
        HStack(spacing: 6) {
          candidateDecision(candidate)
            .lineLimit(1)
          if let first = candidate.startLine, let last = candidate.endLine {
            Text("L\(first)–\(last)").monospacedDigit().lineLimit(1)
          }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
      }
      Spacer(minLength: 2)
      if let slot = candidate.slot {
        Text("Slot \(slot)")
          .font(.caption.monospacedDigit())
          .foregroundStyle(.secondary)
          .fixedSize()
      }
    }
    .padding(.horizontal, 7)
    .padding(.vertical, 6)
    .background(selected ? Color.accentColor.opacity(0.15) : .clear, in: .rect(cornerRadius: 6))
    .contentShape(Rectangle())
  }

  private func candidateDecision(_ candidate: Candidate) -> some View {
    let title: LocalizedStringKey
    let icon: String
    let color: Color
    switch candidate.disposition {
    case .candidateLimit: (title, icon, color) = ("Selection limit", "minus.circle", .secondary)
    case .passageUnavailable:
      (title, icon, color) = ("Unavailable", "exclamationmark.circle", .orange)
    case .fixtureExcluded: (title, icon, color) = ("Test fixture", "minus.circle", .secondary)
    case .contextLimit: (title, icon, color) = ("Input budget", "minus.circle", .secondary)
    case .noUsableDraft:
      (title, icon, color) = ("No usable draft", "exclamationmark.circle", .orange)
    case .notCited: (title, icon, color) = ("Uncited", "text.quote", .secondary)
    case .citationUnverified:
      (title, icon, color) = ("Citation rejected", "xmark.circle", .orange)
    case .citationVerified:
      (title, icon, color) = ("Citation verified", "checkmark.circle", .green)
    case .evidenceChanged:
      (title, icon, color) = ("Evidence changed", "exclamationmark.circle", .orange)
    }
    return Label(title, systemImage: icon)
      .labelStyle(PipelineHeadingStyle(tint: color))
  }

  private func candidateDetail(_ candidate: Candidate) -> some View {
    PipelineGroup("Candidate Details", tint: .blue) {
      if candidate.contextAdded == true {
        Label("Added to context", systemImage: "plus.circle")
          .foregroundStyle(.blue)
      }
      candidateDecision(candidate)
      Text(candidate.disposition.explanation(in: locale)).foregroundStyle(.secondary)
      Text(verbatim: candidate.location?.path ?? candidate.reference)
        .font(.callout.monospaced())
        .textSelection(.enabled)
      if let location = candidate.location {
        Text("Lines \(location.startLine)–\(location.endLine)")
          .font(.caption.monospacedDigit())
          .foregroundStyle(.secondary)
      }
      candidateRankingSignals(candidate)
      HStack {
        if let location = candidate.location, let url = URL(string: location.fileUrl),
          url.host != "benchmark.invalid"
        {
          Link("Open source", destination: url)
        }
        if let slot = candidate.slot,
          answer?.modelContext?.sources.contains(where: { $0.slot == slot }) == true
        {
          Button("Open model slot \(slot)") {
            expandedModelSlots.insert(slot)
            focusedSlot = slot
            stage = .context
          }
          .accessibilityIdentifier("pipeline-ranking-open-model-slot")
        }
      }
      if let text = candidate.text ?? candidate.preview { excerpt(text) }
      if let revision = candidate.revision {
        Text(verbatim: revision).font(.caption.monospaced()).textSelection(.enabled)
      }
      if let location = candidate.location {
        Text("Bytes \(location.startByte)–\(location.endByte) · blob \(location.blob)")
          .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
      }
    }
  }

  @ViewBuilder private func candidateRankingSignals(_ candidate: Candidate) -> some View {
    if let fusion = capture?.fusion {
      Text(
        "Reciprocal rank fusion · offset \(fusion.rankOffset) · vector weight \(fusion.denseWeight.formatted())"
      )
      .font(.caption)
      .foregroundStyle(.secondary)
      if let rank = candidate.fusionRank,
        let fused = fusion.candidates.first(where: { $0.finalRank == rank })
      {
        HStack(spacing: 6) {
          if let lexical = fused.lexicalRank { Text("BM25 #\(lexical)") }
          if let dense = fused.denseRank { Text("Vector #\(dense)") }
          Text("Fusion \(fused.score.formatted(.number.precision(.fractionLength(5))))")
          if fused.lexicalPromoted { Text("Keyword promotion") }
        }
        .font(.caption).foregroundStyle(.secondary)
      }
    }
  }

  private func fileName(_ path: String) -> String {
    path.split(separator: "/").last.map(String.init) ?? path
  }

  @ViewBuilder private func context(_ answer: Answer) -> some View {
    if let modelContext = answer.modelContext {
      ForEach(modelContext.sources, id: \.slot) { source in
        modelInputSlot(source)
      }
    } else {
      Text("No model input was captured for this request.").foregroundStyle(.secondary)
    }
    if let input = capture?.completionInput {
      HStack(alignment: .top) {
        DisclosureGroup("Exact completion input") { excerpt(input) }
        Button("Copy prompt", systemImage: "document.on.document") {
          NSPasteboard.general.clearContents()
          NSPasteboard.general.setString(input, forType: .string)
        }
        .labelStyle(.iconOnly)
        .help("Copy prompt")
      }
    }
  }

  private func modelInputSlot(_ source: Components.Schemas.AnswerModelSource) -> some View {
    let expanded = expandedModelSlots.contains(source.slot)
    return VStack(alignment: .leading, spacing: 9) {
      Button {
        if expanded {
          expandedModelSlots.remove(source.slot)
        } else {
          expandedModelSlots.insert(source.slot)
        }
      } label: {
        HStack {
          Image(systemName: expanded ? "chevron.down" : "chevron.right")
            .font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .frame(width: 12)
          Text("Slot \(source.slot)").font(.callout.weight(.semibold))
          if let path = source.parts.first?.path {
            Text(verbatim: fileName(path))
              .foregroundStyle(.secondary)
              .lineLimit(1)
              .truncationMode(.middle)
          }
          Spacer()
          if let part = source.parts.first, let first = part.startLine, let last = part.endLine {
            Text("L\(first)–\(last)").font(.caption.monospacedDigit()).foregroundStyle(
              .secondary)
          }
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityValue(Text(expanded ? "Expanded" : "Collapsed"))
      .accessibilityIdentifier("answer-pipeline-input-slot-\(source.slot)")

      if expanded {
        ForEach(Array(source.parts.enumerated()), id: \.offset) { _, part in
          if let path = part.path { Text(verbatim: path).textSelection(.enabled) }
          if let first = part.startLine, let last = part.endLine {
            Text("Lines \(first)–\(last)").font(.caption).foregroundStyle(.secondary)
          }
          excerpt(part.text).accessibilityIdentifier("answer-pipeline-input-\(source.slot)")
        }
      }
    }
    .padding(12)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(.quaternary.opacity(0.32), in: .rect(cornerRadius: 8))
    .id(source.slot)
  }

  @ViewBuilder private func generation(_ answer: Answer) -> some View {
    LabeledContent("Generation profile", value: answer.model)
    if let draft = capture?.draft {
      Text(
        verbatim: draft.answer.isEmpty
          ? locale.localized("The model returned an empty draft.") : draft.answer
      )
      .textSelection(.enabled)
      .accessibilityIdentifier("answer-pipeline-draft")
      ForEach(Array(draft.citations.enumerated()), id: \.offset) { _, citation in
        Text("Proposed citation · slot \(citation.passage)").font(.headline)
        excerpt(citation.quote)
      }
    } else {
      Text("No draft was captured for this request.").foregroundStyle(.secondary)
    }
  }

  @ViewBuilder private func verification(_ answer: Answer) -> some View {
    Text("The backend checks each quote against its indexed source.")
      .foregroundStyle(.secondary)
    if let selectedExample { benchmarkReference(selectedExample) }
    PipelineGroup("Outcome", tint: answer.outcome == .answered ? .green : .orange) {
      if answer.outcome == .answered {
        Label("Published", systemImage: "checkmark.circle").foregroundStyle(.green)
        if let text = answer.text { field("Final answer", text) }
      } else {
        Label("Insufficient evidence", systemImage: "exclamationmark.circle")
          .foregroundStyle(.orange).accessibilityIdentifier("answer-pipeline-withheld")
        if let review {
          Text(review.explanation(in: locale)).accessibilityIdentifier("answer-pipeline-reason")
        }
      }
    }
    if !answer.citations.isEmpty {
      PipelineGroup("Citations", tint: .teal) {
        ForEach(Array(answer.citations.enumerated()), id: \.offset) { _, citation in
          let link = citation.lineUrl ?? citation.evidence.location?.fileUrl
          if let link, let url = URL(string: link), url.host != "benchmark.invalid" {
            Link(citation.evidence.title, destination: url)
          } else {
            Text(verbatim: citation.evidence.title).font(.headline)
          }
          if !citation.quote.isEmpty { excerpt(citation.quote) }
        }
      }
    }
  }

  private func field(_ name: LocalizedStringKey, _ value: String) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      Text(name).font(.headline)
      Text(verbatim: value).textSelection(.enabled)
    }
  }

  private func excerpt(_ text: String) -> some View {
    Text(verbatim: text)
      .font(.callout.monospaced())
      .textSelection(.enabled)
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(9)
      .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 6))
  }

  private func restoreSources() async {
    defer { restoringSources = false }
    if let data = UserDefaults.standard.data(forKey: supervisor.pipelinePreferencesKey),
      let saved = try? JSONDecoder().decode(PipelineSourcePreferences.self, from: data)
    {
      sourcePreferences = saved
      importedReference = saved.importedQuestions
      customQuestions = saved.importedQuestions?.questions ?? []
    }
    do {
      let session = try await supervisor.connect(startIfNeeded: false)
      async let corpusRestore: Void = restoreCorpora(session)
      async let recordedRestore: Void = restoreRequestedActions(session)
      do {
        repositories = try await session.githubState().repositories
        if case .repository = scope, selectedRepository == nil { scope = .all }
        var workflows = PipelineWorkflowSources()
        for repository in actionRepositories {
          do {
            let state = try await session.gitHubActions(id: repository.repository.id)
            if let snapshot = state.snapshot, snapshot.branch == repository.branch {
              workflows.load(snapshot)
            }
          } catch {
            setupFeedback = "\(repository.repository.fullName) · \(error.localizedDescription)"
          }
        }
        workflows.restoreSelection(sourcePreferences.workflows)
        sources.workflows = workflows
      } catch {
        setupFeedback = error.localizedDescription
      }
      _ = await (corpusRestore, recordedRestore)
    } catch {
      setupFeedback = error.localizedDescription
    }
  }

  private func restoreCorpora(_ session: SessionClient) async {
    do {
      let saved = try await session.listBenchmarkCorpora()
      sources.savedCorpora = Dictionary(
        uniqueKeysWithValues: saved.map {
          (.init(repository: $0.repository, revision: $0.revision), $0)
        })
      sources.loadedCorpora = sources.savedCorpora.filter { $0.value.active == true }
    } catch {
      sources.savedCorpora = [:]
      sources.loadedCorpora = [:]
      importFeedback = error.localizedDescription
    }
  }

  private func restoreRequestedActions(_ session: SessionClient) async {
    do {
      let downloads = try await session.benchmarkActionDownloads()
      sources.actionDownloads = Dictionary(
        uniqueKeysWithValues: downloads.map { ($0.reference, $0) })
      rebuildDownloadedWorkflows()
    } catch {
      setupFeedback = error.localizedDescription
    }
    if sourcePreferences.loadedCaptures == true || sourcePreferences.recordedWorkflows != nil {
      do {
        try await loadRecordedActions(session)
      } catch {
        setupFeedback = error.localizedDescription
      }
    }
  }

  private func rebuildDownloadedWorkflows(activatingNewSince previous: Set<String>? = nil) {
    let snapshots = sources.actionDownloads.values.flatMap(\.snapshots)
    var workflows = PipelineWorkflowSources()
    workflows.loadAll(snapshots)
    let all = Set(workflows.all.map(\.id))
    var active = sourcePreferences.downloadedWorkflows ?? all
    if let previous {
      active.formUnion(all.subtracting(previous))
      sourcePreferences.downloadedWorkflows = active
      saveSourcePreferences()
    }
    workflows.restoreSelection(active)
    sources.downloadedWorkflows = workflows
  }

  private func loadRecordedActions(_ session: SessionClient) async throws {
    let snapshots = try await session.recordedBenchmarkActions()
    var workflows = PipelineWorkflowSources()
    workflows.loadAll(snapshots)
    let identifiers =
      sourcePreferences.recordedWorkflows
      ?? Set(snapshots.flatMap { workflows.runs(for: $0.repositoryId) ?? [] }.map(\.id))
    workflows.restoreSelection(identifiers)
    sources.recordedWorkflows = workflows
    sources.recordedSnapshots = snapshots
  }

  private func saveSourcePreferences() {
    if let data = try? JSONEncoder().encode(sourcePreferences) {
      UserDefaults.standard.set(data, forKey: supervisor.pipelinePreferencesKey)
    }
  }

  private func run() {
    let submitted = query
    guard requestID == nil, canRun else { return }
    let id = UUID()
    let repository = selectedRepository
    let files = scope == .temporary ? documents : []
    let reference = referenceCorpus?.source
    let workflow = selectedWorkflow
    let origin = workflowOrigin
    let retrieval = benchmarkRetrieval
    submittedBenchmarkBM25 = reference != nil && retrieval == .bm25
    submittedWorkflow = workflow
    submittedCorpus = readyReference
    submittedFiles = files.map(\.path)
    submittedBytes = files.reduce(0) { $0 + $1.text.utf8.count }
    requestID = id
    indexingBenchmark = reference != nil && retrieval == .hybrid
    indexProgress = nil
    submittedQuestion = submitted
    submittedRepository = repository
    response = nil
    feedback = nil
    hybridUnavailable = false
    selectedCandidate = nil
    expandedRetrievalCandidate = nil
    focusedSlot = nil
    expandedModelSlots.removeAll()
    let request = InspectionRequest(
      query: submitted, id: id, repository: repository, files: files, reference: reference,
      workflow: workflow, origin: origin, retrieval: retrieval)
    Task { await execute(request) }
  }

  private func execute(_ request: InspectionRequest) async {
    defer {
      requestID = nil
      cancelling = false
      indexingBenchmark = false
    }
    do {
      let session = try await supervisor.connect(startIfNeeded: false)
      response = try await inspect(request, session: session)
    } catch {
      guard !cancelling || !indexingBenchmark else { return }
      if let reference = request.reference,
        (error as? AnswerRejection)?.problemCode == "benchmark_corpus_not_loaded"
      {
        sources.loadedCorpora.removeValue(forKey: reference)
      }
      hybridUnavailable =
        request.reference != nil && request.retrieval == .hybrid
        && PipelineBenchmark.embeddingUnavailable(error)
      feedback = error.localizedDescription
    }
  }

  private func inspect(_ request: InspectionRequest, session: SessionClient) async throws
    -> AnswerResponse?
  {
    if let reference = request.reference {
      if request.retrieval == .hybrid {
        try await indexBenchmarkCorpus(reference, session: session)
        guard !cancelling else { return nil }
        indexingBenchmark = false
      }
      return try await session.askBenchmark(
        request.query, id: request.id, reference: reference, retrieval: request.retrieval)
    }
    if let workflow = request.workflow {
      return try await inspectWorkflow(workflow, request: request, session: session)
    }
    if request.files.isEmpty {
      return try await session.askWithTrace(
        request.query, id: request.id, repositoryId: request.repository?.repository.id,
        branch: request.repository?.branch, inspectPipeline: true)
    }
    return try await session.askBenchmark(
      request.query, id: request.id, documents: request.files)
  }

  private func indexBenchmarkCorpus(
    _ reference: Components.Schemas.BenchmarkReference, session: SessionClient
  ) async throws {
    while !cancelling {
      let progress = try await session.embedBenchmarkCorpusBatch(reference)
      indexProgress = progress
      if progress.state == .ready { break }
    }
  }

  private func inspectWorkflow(
    _ workflow: PipelineWorkflow, request: InspectionRequest, session: SessionClient
  ) async throws -> AnswerResponse {
    if let origin = request.origin, origin != .connected {
      return try await session.askBenchmark(
        request.query, id: request.id,
        action: .init(
          repository: workflow.repository, runId: workflow.runID, attempt: workflow.attempt,
          downloaded: origin == .downloaded))
    }
    return try await session.askWithTrace(
      request.query, id: request.id, repositoryId: workflow.repositoryID, branch: workflow.branch,
      actionsRunId: workflow.runID, actionsRunAttempt: workflow.attempt, inspectPipeline: true)
  }

  private func cancel() {
    guard let requestID, !cancelling else { return }
    cancelling = true
    if indexingBenchmark { return }
    Task {
      do {
        try await supervisor.connect(startIfNeeded: false).cancelAnswer(requestID)
      } catch {
        if self.requestID == requestID {
          feedback = error.localizedDescription
          cancelling = false
        }
      }
    }
  }
}

private struct PipelineHeadingStyle: LabelStyle {
  var tint: Color

  func makeBody(configuration: Configuration) -> some View {
    HStack(spacing: 8) {
      configuration.icon.foregroundStyle(tint)
      configuration.title
    }
  }
}

private struct PipelineGroup<Content: View>: View {
  var title: LocalizedStringKey
  var tint: Color
  var content: Content

  init(_ title: LocalizedStringKey, tint: Color, @ViewBuilder content: () -> Content) {
    self.title = title
    self.tint = tint
    self.content = content()
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 9) {
      Text(title).font(.headline).foregroundStyle(tint).accessibilityAddTraits(.isHeader)
      content
    }
    .labeledContentStyle(PipelineMetadataStyle())
    .padding(14)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(.quaternary.opacity(0.36), in: .rect(cornerRadius: 9))
  }
}

private struct PipelineMetadataStyle: LabeledContentStyle {
  func makeBody(configuration: Configuration) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 12) {
      configuration.label
        .foregroundStyle(.secondary)
        .frame(width: 190, alignment: .leading)
      configuration.content
      Spacer(minLength: 0)
    }
  }
}
