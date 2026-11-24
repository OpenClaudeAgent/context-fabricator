import AppKit
import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI
import UniformTypeIdentifiers

struct InferenceSetupView: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<InferenceSetup>
  let profiles: [Components.Schemas.ModelDownload]
  let catalogFeedback: String?
  let runtime: Components.Schemas.RuntimeStatus?
  let manageDownloads: () -> Void
  @State private var answerProfile = ""
  @State private var confirmedProfile: String?
  @Environment(\.dismiss) private var dismiss
  @State private var embedding = false
  @State private var embeddingProfile = ""
  @State private var confirmedEmbeddingProfile: String?
  @State private var source: Components.Schemas.InferenceSetupRuntimeSource
  @State private var archive: URL?
  @State private var cancellingWaitingSetup = false
  @State private var waitingOperationToCancel: String?
  @State private var settingUpAgain = false

  init(
    store: StoreOf<InferenceSetup>, profiles: [Components.Schemas.ModelDownload],
    catalogFeedback: String?, runtime: Components.Schemas.RuntimeStatus?,
    manageDownloads: @escaping () -> Void
  ) {
    self.store = store
    self.profiles = profiles
    self.catalogFeedback = catalogFeedback
    self.runtime = runtime
    self.manageDownloads = manageDownloads
    let hostedAvailable = runtime?.state == .ready || runtime?.canDownload == true
    _source = State(initialValue: hostedAvailable ? .hosted : .localArchive)
  }

  private var hostedAvailable: Bool { runtime?.state == .ready || runtime?.canDownload == true }
  private var archiveAvailable: Bool { runtime?.archiveBytes != nil }

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text("Set Up Answers")
        .font(.title2.weight(.semibold))
        .accessibilityAddTraits(.isHeader)
      setupContent
      Divider().padding(.top, 2)
      footer
    }
    .padding(24).frame(width: 480, height: 480)
    .alert(
      "Cancel Waiting Setup?", isPresented: $cancellingWaitingSetup,
      presenting: waitingOperationToCancel
    ) { operationID in
      Button("Keep Waiting", role: .cancel) {}
      Button("Cancel Setup", role: .destructive) { store.send(.cancelArchive(operationID)) }
    } message: { _ in
      Text("The waiting setup will be cancelled. Already installed components are retained.")
    }
    .onAppear { selectRecommendedIfNeeded() }
    .onChange(of: profiles) {
      selectRecommendedIfNeeded()
      if !selectionAvailable { store.send(.cancel) }
    }
    .onChange(of: answerProfile) { store.send(.cancel) }
    .onChange(of: embedding) { store.send(.cancel) }
    .onChange(of: embeddingProfile) { store.send(.cancel) }
    .onChange(of: source) { store.send(.cancel) }
    .onChange(of: store.inProgress) {
      if store.inProgress { settingUpAgain = false }
    }
    .onDisappear { store.send(.cancel) }
  }

  @ViewBuilder private var setupContent: some View {
    if store.starting {
      LoadingStateView("Starting \(modelLabel(confirmedProfile ?? answerProfile))…")
    } else if store.requesting && !store.inProgress {
      LoadingStateView("Checking setup requirements…")
    } else if waitingForChoices {
      LoadingStateView("Loading model choices…")
    } else {
      ScrollView {
        VStack(alignment: .leading, spacing: 14) { setupDetails }
          .frame(maxWidth: .infinity, alignment: .leading)
      }
      .defaultScrollAnchor(.top)
    }
  }

  private var waitingForChoices: Bool {
    profiles.isEmpty && catalogFeedback == nil && !store.inProgress
      && !showingResult && store.plan == nil
  }

  @ViewBuilder private var setupDetails: some View {
    if !store.inProgress && !store.starting && !showingResult && store.plan == nil {
      Text("Choose a local model to answer questions from your sources.")
        .foregroundStyle(.secondary)
      VStack(alignment: .leading, spacing: 7) {
        Text("Inference engine").font(.headline).accessibilityAddTraits(.isHeader)
        if runtime?.state == .ready {
          runtimeReadiness
        } else if hostedAvailable || archiveAvailable {
          Picker("Runtime source", selection: $source) {
            if hostedAvailable {
              Text("GitHub Releases").tag(Components.Schemas.InferenceSetupRuntimeSource.hosted)
            }
            if archiveAvailable {
              Text("Local ZIP").tag(Components.Schemas.InferenceSetupRuntimeSource.localArchive)
            }
          }
          .disabled(store.requesting || store.inProgress)
          if source == .localArchive && archiveAvailable {
            HStack {
              Button("Choose Runtime ZIP…") { chooseArchive() }
                .disabled(store.requesting || store.inProgress)
              Text(archive?.lastPathComponent ?? locale.localized("No archive selected"))
                .foregroundStyle(.secondary)
            }
          }
        } else {
          Label("Local inference engine is unavailable.", systemImage: "exclamationmark.circle")
            .foregroundStyle(.secondary)
        }
      }
      Divider()
      if setupCatalogReady {
        answerChoices
        if !embeddingModels.isEmpty || embedding {
          Divider()
          semanticSearchChoice
        }
      } else if let catalogFeedback {
        Label(
          "Unable to load model choices: \(catalogFeedback) Retrying…",
          systemImage: "exclamationmark.circle"
        )
        .font(.callout).foregroundStyle(.secondary)
      } else {
        Label("Recommended model choices are unavailable.", systemImage: "exclamationmark.circle")
          .font(.callout).foregroundStyle(.secondary)
      }
      if setupCatalogReady && !selectionAvailable {
        Label(
          "Setup is unavailable. Refresh the estimate and try again.",
          systemImage: "exclamationmark.circle"
        )
        .font(.callout).foregroundStyle(.secondary)
      }
    }
    if let plan = store.plan {
      review(plan)
    }
    if let status = store.status, !settingUpAgain, store.plan == nil {
      statusContent(status)
      if let error = status.error {
        Text(locale.localizedKey(error)).font(.caption).foregroundStyle(.secondary)
      }
      if status.phase == .waitingForArchive {
        HStack {
          Button("Choose Runtime ZIP…") { chooseRecoveryArchive(operationID: status.operationId) }
          Button("Cancel Waiting Setup…", role: .destructive) {
            waitingOperationToCancel = status.operationId
            cancellingWaitingSetup = true
          }
        }.disabled(store.requesting)
      }
    }
    if let feedback = store.feedback {
      Label(locale.localizedKey(feedback), systemImage: "exclamationmark.circle").font(.caption)
    }
  }

  private var answerChoices: some View {
    VStack(alignment: .leading, spacing: 7) {
      Text("Answer model").font(.headline).accessibilityAddTraits(.isHeader)
      Picker("Answer model", selection: $answerProfile) {
        ForEach(answerModels, id: \.profile) { model in
          Text(optionLabel(model)).tag(model.profile)
            .accessibilityIdentifier("setup-choice-\(model.profile)")
        }
      }
      .pickerStyle(.radioGroup)
      .labelsHidden()
      .disabled(store.requesting)
      if let source = answerModels.first(where: { $0.profile == answerProfile })?.source {
        Label(source.label, systemImage: "globe")
          .font(.callout).foregroundStyle(.secondary)
      }
    }
  }

  private var semanticSearchChoice: some View {
    VStack(alignment: .leading, spacing: 7) {
      Text("Semantic search").font(.headline).accessibilityAddTraits(.isHeader)
      Toggle("Add semantic search", isOn: $embedding)
        .toggleStyle(.switch)
        .disabled(store.requesting)
        .accessibilityIdentifier("setup-optional-embedding")
      if embedding {
        Picker("Semantic search", selection: $embeddingProfile) {
          Text("Automatic").tag("")
          ForEach(embeddingModels, id: \.profile) { model in
            Text(optionLabel(model)).tag(model.profile)
              .accessibilityIdentifier("setup-choice-\(model.profile)")
          }
        }
        .pickerStyle(.radioGroup)
        .labelsHidden()
        .disabled(store.requesting)
        if let source = embeddingModels.first(where: { $0.profile == embeddingProfile })?.source {
          Label(source.label, systemImage: "globe")
            .font(.callout).foregroundStyle(.secondary)
        }
      }
    }
  }

  @ViewBuilder private var runtimeReadiness: some View {
    if let engine = runtime?.engine {
      HStack(spacing: 8) {
        Image(systemName: "checkmark.circle").accessibilityHidden(true)
        Text(verbatim: engine.name).accessibilityIdentifier("setup-runtime-engine-name")
        Text("Build \(engine.build)")
          .monospacedDigit()
          .accessibilityIdentifier("setup-runtime-engine-build")
        Spacer(minLength: 0)
        Text(archiveAvailable ? "Ready" : "Included")
      }
      .font(.callout)
      .foregroundStyle(.secondary)
    } else {
      Label(
        archiveAvailable ? "Local inference engine ready" : "Local inference engine included",
        systemImage: "checkmark.circle"
      )
      .foregroundStyle(.secondary)
    }
  }

  private var showingResult: Bool {
    store.status != nil && !store.inProgress && !store.starting && !settingUpAgain
  }

  private var footer: some View {
    HStack {
      if showingResult {
        if canManageDownloads {
          manageDownloadsButton
        } else {
          Button("Set Up Again", action: editSelection)
        }
        Spacer()
        if canManageDownloads {
          Button("Review Options", action: editSelection)
        }
        Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
      } else {
        if store.plan != nil {
          Button("Back") { store.send(.cancel) }
        } else if canManageDownloads {
          manageDownloadsButton
        }
        Spacer()
        Button(closeTitle) {
          store.send(.cancel)
          dismiss()
        }
        .keyboardShortcut(.cancelAction)
        if let plan = store.plan {
          Button(plan.maximumTransferBytes == 0 ? "Enable Model" : "Download and Enable") {
            confirmedProfile = plan.selection.answerProfile ?? choices.defaultAnswer
            confirmedEmbeddingProfile = plan.selection.embeddingProfile
            store.send(.confirm)
          }
          .keyboardShortcut(.defaultAction)
          .disabled(
            store.requesting || !plan.canStart
              || (plan.runtimeRequired && source == .localArchive && archive == nil))
        } else if !store.inProgress && !store.starting {
          Button("Review Setup") {
            store.send(
              .review(
                .init(
                  answerProfile: answerProfile, embedding: embedding,
                  embeddingProfile: selectedEmbeddingProfile, runtimeSource: source),
                archive?.path))
          }
          .keyboardShortcut(.defaultAction)
          .disabled(
            store.requesting || !setupCatalogReady || !selectionAvailable
              || (!hostedAvailable && !archiveAvailable))
        }
      }
    }
  }

  private var manageDownloadsButton: some View {
    Button("Manage Downloads…", action: manageDownloads)
      .accessibilityIdentifier("setup-manage-downloads")
  }

  private func editSelection() {
    if let selection = store.status?.selection {
      if let profile = selection.answerProfile { answerProfile = profile }
      embedding = selection.embedding
      embeddingProfile = selection.embeddingProfile ?? ""
    }
    settingUpAgain = true
  }

  private var canManageDownloads: Bool {
    guard !settingUpAgain else { return false }
    return switch store.status?.phase {
    case .answerWeights, .embeddingWeights, .failed, .interrupted: true
    default: false
    }
  }

  private func review(_ plan: Components.Schemas.InferenceSetupPlan) -> some View {
    let profile = plan.selection.answerProfile ?? choices.defaultAnswer
    return VStack(alignment: .leading, spacing: 12) {
      reviewModel("Answer model", profile: profile, installed: plan.answerInstalled)
      if plan.selection.embedding {
        reviewModel(
          "Semantic search",
          profile: plan.selection.embeddingProfile,
          installed: plan.embeddingInstalled == true)
      }
      if plan.runtimeRequired {
        LabeledContent(
          "Inference engine", value: source == .hosted ? "GitHub Releases" : "Local ZIP")
        if source == .localArchive {
          Button("Choose Runtime ZIP…") { chooseArchive() }
          Text(
            archive?.lastPathComponent ?? locale.localized("Choose a runtime ZIP before starting.")
          )
          .foregroundStyle(.secondary)
        }
      }
      Divider()
      LabeledContent("Total download", value: bytes(plan.maximumTransferBytes))
      LabeledContent("Peak extra disk space", value: bytes(plan.peakAdditionalBytes))
        .accessibilityIdentifier("setup-space-estimate")
      LabeledContent("Available space", value: bytes(plan.availableBytes))
      if let blocker = plan.blocker {
        Label(blockerMessage(blocker), systemImage: "exclamationmark.circle")
          .foregroundStyle(.red)
      }
    }
    .font(.callout)
    .labeledContentStyle(WorkspaceRowStyle())
    .padding(12)
    .background(.quaternary, in: .rect(cornerRadius: 8))
  }

  private func reviewModel(
    _ role: LocalizedStringKey, profile: String?, installed: Bool
  ) -> some View {
    let model = profiles.first { $0.profile == profile }
    return LabeledContent {
      VStack(alignment: .trailing, spacing: 3) {
        Text(modelLabel(profile))
          .fontWeight(.medium)
          .accessibilityIdentifier(
            "setup-reviewed-\(model?.kind == .embedding ? "embedding" : "answer")")
        if let model {
          Text(installed ? locale.localized("Already installed") : bytes(model.totalBytes))
            .foregroundStyle(.secondary)
          if let source = model.source, let url = URL(string: source.url) {
            Link(source.label, destination: url).font(.caption)
          }
        }
      }
    } label: {
      Text(role)
    }
  }

  private var closeTitle: LocalizedStringKey {
    if store.status?.phase == .waitingForArchive { return "Close" }
    if store.inProgress { return "Continue in Background" }
    return store.starting ? "Close" : "Cancel"
  }

  private var recommendedModel: Components.Schemas.ModelDownload? {
    profiles.first { $0.kind == .generation && $0.setupRole == .recommendedAnswer }
  }
  private var choices: InferenceSetupChoices { InferenceSetupChoices(profiles: profiles) }
  private var answerModels: [Components.Schemas.ModelDownload] { choices.answers }
  private var embeddingModels: [Components.Schemas.ModelDownload] { choices.embeddings }
  private var setupCatalogReady: Bool { !answerModels.isEmpty }
  private var selectedEmbeddingProfile: String? {
    embedding && !embeddingProfile.isEmpty ? embeddingProfile : nil
  }
  private var selectionAvailable: Bool {
    choices.contains(answer: answerProfile, embedding: selectedEmbeddingProfile)
  }
  private func selectRecommendedIfNeeded() {
    guard !profiles.isEmpty else { return }
    if answerProfile.isEmpty {
      answerProfile = recommendedModel?.profile ?? answerModels.first?.profile ?? ""
    }
  }
  private func optionLabel(_ model: Components.Schemas.ModelDownload) -> String {
    let name = model.label ?? model.profile
    let size = bytes(model.totalBytes)
    return model.setupRole == .recommendedAnswer
      ? locale.localized("\(name) · Recommended · \(size)") : "\(name) · \(size)"
  }
  private func modelLabel(_ profile: String?) -> String {
    guard let profile, !profile.isEmpty else { return locale.localized("Preparing model…") }
    return profiles.first { $0.profile == profile }.map { $0.label ?? $0.profile } ?? profile
  }
  private func blockerMessage(_ blocker: String) -> String {
    switch blocker {
    case "runtime_unavailable": locale.localized("The local inference engine is unavailable.")
    case "runtime_busy": locale.localized("The local inference engine is being installed.")
    case "model_job_busy":
      locale.localized("A selected model download is already in progress or paused.")
    case "runtime_download_unavailable":
      locale.localized("The local inference engine cannot be downloaded.")
    case "storage_limit": locale.localized("There is not enough available storage for this setup.")
    default: locale.localized("Setup is unavailable. Refresh the estimate and try again.")
    }
  }

  @ViewBuilder private func statusContent(_ status: Components.Schemas.InferenceSetupStatus)
    -> some View
  {
    let selected = status.selection?.answerProfile ?? confirmedProfile ?? choices.defaultAnswer
    let downloading = status.phase == .answerWeights || status.phase == .embeddingWeights
    let stopped = status.phase == .failed || status.phase == .interrupted
    let isEmbedding =
      status.phase == .embeddingWeights
      || (stopped && status.answerReady && status.selection?.embedding == true)
    let activeProfile =
      isEmbedding
      ? (status.selection?.embeddingProfile ?? confirmedEmbeddingProfile)
      : selected
    let model = profiles.first { $0.profile == activeProfile }
    VStack(alignment: .leading, spacing: 10) {
      Text(modelLabel(activeProfile)).font(.headline)
        .accessibilityIdentifier("setup-active-model")
      if let model, downloading || (stopped && model.state == .paused) {
        downloadProgress(for: model, isEmbedding: isEmbedding)
      } else if store.inProgress && status.phase != .waitingForArchive {
        ProgressView(statusText(status)).progressViewStyle(.linear)
      } else {
        Text(statusText(status)).font(.callout).foregroundStyle(.secondary)
      }
    }
  }

  private func downloadProgress(
    for model: Components.Schemas.ModelDownload, isEmbedding: Bool
  ) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack(alignment: .firstTextBaseline) {
        Text(transferLabel(model, isEmbedding: isEmbedding))
        Spacer(minLength: 12)
        if model.showsTransferBytes {
          Text("\(bytes(model.downloadedBytes)) of \(bytes(model.totalBytes))")
            .monospacedDigit()
            .accessibilityIdentifier("setup-progress-bytes")
        }
      }
      .font(.callout)
      if model.state == .installing || model.state == .pausing || model.state == .notInstalled {
        ModelTransferProgress(model: model)
          .accessibilityIdentifier("setup-download-progress")
      }
      if let source = model.source, let url = URL(string: source.url) {
        Link(destination: url) { Label(source.label, systemImage: "globe") }
          .font(.caption)
          .accessibilityIdentifier("setup-download-source")
      }
    }
    .padding(12)
    .background(.quaternary, in: .rect(cornerRadius: 8))
  }

  private func transferLabel(_ model: Components.Schemas.ModelDownload, isEmbedding: Bool) -> String
  {
    switch model.state {
    case .notInstalled: locale.localized("Preparing download…")
    case .installing:
      switch model.installationStage {
      case .verifying: locale.localized("Verifying model file…")
      case .preparing: locale.localized("Preparing download…")
      case .transferring, .none:
        isEmbedding
          ? locale.localized("Downloading embeddings")
          : locale.localized("Downloading answer model")
      }
    case .pausing: locale.localized("Pausing download…")
    case .paused: locale.localized("Download paused")
    case .interrupted: locale.localized("Download interrupted")
    case .failed: locale.localized("Download failed")
    case .installed: locale.localized("Download complete")
    }
  }

  private func chooseArchive() {
    if let url = pickRuntimeZIP() {
      store.send(.cancel)
      archive = url
    }
  }

  private func chooseRecoveryArchive(operationID: String) {
    if let url = pickRuntimeZIP() {
      store.send(.provideArchive(operationID, url.path))
    }
  }

  private func pickRuntimeZIP() -> URL? {
    let panel = NSOpenPanel()
    panel.allowedContentTypes = [.zip]
    panel.allowsMultipleSelection = false
    panel.canChooseDirectories = false
    return panel.runModal() == .OK ? panel.url : nil
  }

  private func bytes(_ count: Int64) -> String {
    count.formatted(.byteCount(style: .file).locale(locale))
  }
  private func statusText(_ status: Components.Schemas.InferenceSetupStatus) -> String {
    switch status.phase {
    case .waitingForArchive: locale.localized("Choose the runtime ZIP again to continue setup.")
    case .runtime: locale.localized("Setting up the runtime…")
    case .answerWeights: locale.localized("Downloading answer weights…")
    case .activatingAnswer: locale.localized("Activating the answer model…")
    case .embeddingWeights: locale.localized("Downloading embedding weights…")
    case .completed:
      status.answerReady
        ? locale.localized("Setup completed. Answers are ready.")
        : locale.localized("Setup completed.")
    case .failed:
      status.answerReady
        ? locale.localized("Setup stopped. Answers are ready.")
        : locale.localized("Setup failed.")
    case .interrupted:
      status.answerReady
        ? locale.localized("Setup interrupted. Answers are ready.")
        : locale.localized("Setup interrupted.")
    }
  }
}

struct DirectInferenceSetupView: View {
  let store: StoreOf<ModelLibrary>
  let manageDownloads: () -> Void
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    Group {
      if let runtime = store.runtime {
        InferenceSetupView(
          store: store.scope(state: \.setup, action: \.setup),
          profiles: store.profiles ?? [], catalogFeedback: store.catalogFeedback,
          runtime: runtime, manageDownloads: manageDownloads)
      } else if let error = store.runtimeStatusFeedback {
        VStack {
          ContentUnavailableView(
            "Unable to check the local engine", systemImage: "exclamationmark.circle",
            description: Text(error)
          )
          Button("Close") { dismiss() }
        }
        .padding(24)
      } else {
        LoadingStateView("Checking the local engine…")
      }
    }
    .frame(width: 480, height: 480)
    .task { await store.send(.observe).finish() }
    .onDisappear { store.send(.stopObserving) }
  }
}
