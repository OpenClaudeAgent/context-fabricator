import AppKit
import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI
import UniformTypeIdentifiers

struct ModelLibraryView: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<ModelLibrary>
  let inferenceDirectory: URL
  let connection: Dashboard.Connection
  private var backendReady: Bool { connection == .ready }
  let restartBlocked: Bool
  let restart: () -> Void
  @Environment(\.dismiss) private var dismiss
  @State private var showingSetup = false
  @State private var modelFolderError = false
  @State private var query = ""
  @FocusState private var searchFocused: Bool
  @State private var confirmingIndexDeletion = false
  @State private var confirmingDiscard = false
  @State private var selectedPartial: Components.Schemas.ModelDownload?

  var body: some View {
    Group {
      if store.showingDiscovery {
        HubCatalogView(store: store.scope(state: \.hub, action: \.hub))
      } else {
        modelLibrary
      }
    }
    .sheet(isPresented: $showingSetup) {
      InferenceSetupView(
        store: store.scope(state: \.setup, action: \.setup),
        profiles: store.profiles ?? [], catalogFeedback: store.catalogFeedback,
        runtime: store.runtime,
        manageDownloads: { showingSetup = false })
    }
    .frame(width: librarySize.width, height: librarySize.height)
    .task(id: backendReady) {
      if backendReady { await store.send(.observe).finish() }
    }
    .onDisappear {
      store.send(.stopObserving)
      if store.showingDiscovery { store.send(.hub(.close)) }
    }
  }

  private var librarySize: CGSize {
    let visible = NSScreen.main?.visibleFrame.size ?? CGSize(width: 1024, height: 768)
    return CGSize(
      width: min(820, max(1, visible.width - 48)),
      height: min(760, max(1, visible.height - 48)))
  }

  private var modelLibrary: some View {
    VStack(alignment: .leading, spacing: 0) {
      header
        .padding(.horizontal, 24)
        .padding(.top, 22)
        .padding(.bottom, 14)

      libraryContents
        .frame(maxWidth: .infinity, maxHeight: .infinity)

      Divider()
      HStack(spacing: 8) {
        Spacer()
        Button("Open Model Folder", systemImage: "folder", action: showModelFolder)
        if connection != .connecting
          && (!backendReady || store.runtime?.state != .ready || store.runtime?.archiveBytes != nil)
        {
          Button(
            backendReady ? "Restart Backend" : "Retry Connection", systemImage: "arrow.clockwise",
            action: restart
          )
          .disabled(backendReady && (restartBlocked || transferActive))
          .accessibilityIdentifier("model-library-retry-backend")
        }
        Button("Done") { dismiss() }
          .keyboardShortcut(.cancelAction)
          .buttonStyle(.borderedProminent)
      }
      .controlSize(.regular)
      .padding(.horizontal, 24)
      .padding(.vertical, 13)
      .background(.bar)
    }
    .alert("Delete Embedding Index?", isPresented: $confirmingIndexDeletion) {
      Button("Cancel", role: .cancel) {}
      Button("Delete Index", role: .destructive) { store.send(.deleteIndex) }
    } message: {
      Text(
        "This removes embedding vectors. Your documents and lexical search remain available. The index can be rebuilt when enabled."
      )
    }
    .alert("Couldn’t Open the Model Folder", isPresented: $modelFolderError) {
      Button("OK", role: .cancel) {}
    }
    .alert(
      "Discard Partial Download?", isPresented: $confirmingDiscard, presenting: selectedPartial
    ) { model in
      Button("Cancel", role: .cancel) {}
      Button("Discard Partial Download", role: .destructive) {
        store.send(.discardPartial(model.profile))
      }
    } message: { model in
      let size = bytes(model.downloadedBytes)
      Text(
        "Discard \(size) of partial data for \(model.label ?? model.profile)? Installed weights are retained."
      )
    }
    .alert(
      "Delete Model?",
      isPresented: Binding(
        get: { store.removalPlan != nil },
        set: { if !$0 { store.send(.cancelRemoval) } }
      ), presenting: store.removalPlan
    ) { _ in
      Button("Cancel", role: .cancel) { store.send(.cancelRemoval) }
      Button("Delete Model", role: .destructive) { store.send(.confirmRemoval) }
    } message: { plan in
      Text(
        "\(store.feedback.map { locale.localizedKey($0) + "\n\n" } ?? "")Delete the local weights for \(plan.profile) (\(bytes(plan.bytes)) of logical data)? You can download them again."
      )
    }
  }

  @ViewBuilder private var libraryContents: some View {
    if connection == .connecting {
      LoadingStateView("Connecting to the local backend…")
    } else if backendReady && store.profiles == nil && store.catalogFeedback == nil {
      LoadingStateView("Loading the model library")
    } else {
      ScrollView {
        VStack(alignment: .leading, spacing: 16) {
          if store.profiles != nil { search }
          catalog
          storageSummary
          runtimeControls
          indexControls
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 24)
        .padding(.bottom, 18)
      }
      .defaultScrollAnchor(.top)
      .accessibilityIdentifier("model-library-content")
    }
  }

  private var header: some View {
    HStack {
      Text("Manage Models")
        .font(.title2.weight(.semibold))
        .accessibilityAddTraits(.isHeader)
      Spacer()
      if store.runtime != nil {
        Button("Set Up Answers…", systemImage: "sparkles") { showingSetup = true }
          .disabled(!backendReady)
      }
      Button("Search Models…", systemImage: "magnifyingglass") { store.send(.openDiscovery) }
        .disabled(!backendReady || transferActive)
        .help("Search Hugging Face")
        .accessibilityIdentifier("browse-hugging-face")
    }
  }

  private var search: some View {
    HStack(spacing: 8) {
      Button {
        searchFocused = true
      } label: {
        Image(systemName: "magnifyingglass")
      }
      .buttonStyle(.plain)
      .foregroundStyle(.secondary)
      .keyboardShortcut("f", modifiers: .command)
      .accessibilityLabel("Focus model search")
      TextField("Filter library…", text: $query)
        .textFieldStyle(.roundedBorder)
        .focused($searchFocused)
        .accessibilityIdentifier("model-library-search")
      if !query.isEmpty {
        Button {
          query = ""
        } label: {
          Image(systemName: "xmark.circle.fill")
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .accessibilityLabel("Clear Search")
      }
    }
  }

  private func matching(_ profiles: [Components.Schemas.ModelDownload])
    -> [Components.Schemas.ModelDownload]
  {
    let term = query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !term.isEmpty else { return profiles }
    return profiles.filter { model in
      [
        model.label, model.profile, model.kind?.rawValue, model.source?.label,
        model.source?.repository,
      ]
      .compactMap { $0 }
      .contains { $0.localizedStandardContains(term) }
    }
  }

  @ViewBuilder private var catalog: some View {
    if let message = connection.failureMessage {
      Label(
        "Unable to connect to the local backend: \(locale.localizedKey(message))",
        systemImage: "exclamationmark.circle"
      )
      .foregroundStyle(.secondary)
    } else if let profiles = store.profiles {
      catalogResults(profiles)
    }
    if backendReady, let catalogFeedback = store.catalogFeedback {
      Label(catalogFeedback, systemImage: "exclamationmark.circle")
        .foregroundStyle(.secondary)
    }
    if store.feedbackProfile == nil, let feedback = store.feedback {
      Label(locale.localizedKey(feedback), systemImage: "exclamationmark.circle")
        .foregroundStyle(.secondary)
        .accessibilityIdentifier("model-library-feedback")
    }
  }

  private func catalogResults(_ profiles: [Components.Schemas.ModelDownload]) -> some View {
    let groups = Dictionary(grouping: profiles, by: \.isDownload)
    let downloads = groups[true] ?? []
    let results = matching(groups[false] ?? [])
    return VStack(alignment: .leading, spacing: 12) {
      modelSection("Downloads", models: downloads, headingID: "model-library-downloads")
      if results.isEmpty && !query.isEmpty {
        Label("No catalog models match your search.", systemImage: "magnifyingglass")
          .font(.callout).foregroundStyle(.secondary)
      } else if profiles.isEmpty {
        Label("No models are available.", systemImage: "cpu")
          .font(.callout).foregroundStyle(.secondary)
      } else {
        modelSections(results)
      }
    }
  }

  @ViewBuilder private func modelSections(_ models: [Components.Schemas.ModelDownload]) -> some View
  {
    modelSection("Answer models", models: sortedModels(models.filter { $0.kind == .generation }))
    modelSection("Embedding models", models: sortedModels(models.filter { $0.kind == .embedding }))
    modelSection("Available models", models: sortedModels(models.filter { $0.kind == nil }))
  }

  private func sortedModels(_ models: [Components.Schemas.ModelDownload])
    -> [Components.Schemas.ModelDownload]
  {
    models.filter { $0.state == .installed } + models.filter { $0.state != .installed }
  }

  @ViewBuilder private func modelSection(
    _ title: LocalizedStringKey, models: [Components.Schemas.ModelDownload],
    headingID: String? = nil
  ) -> some View {
    if !models.isEmpty {
      librarySection(title, headingID: headingID) {
        VStack(spacing: 0) {
          ForEach(models, id: \.profile) { model in
            if model.profile != models.first?.profile { Divider().padding(.horizontal, 12) }
            ModelDownloadRow(
              model: model,
              storage: store.storagePlan?.models.first { $0.profile == model.profile },
              requesting: store.pendingProfile == model.profile,
              requestPending: store.pendingProfile != nil,
              requestFeedback: store.feedbackProfile == model.profile ? store.feedback : nil
            ) {
              store.send(.install(model.profile))
            } pause: {
              store.send(.pause(model.profile))
            } resume: {
              store.send(.resume(model.profile))
            } discard: {
              selectedPartial = model
              confirmingDiscard = true
            } delete: {
              store.send(.prepareRemoval(model.profile))
            }
          }
        }
      }
    }
  }

  private func librarySection<Content: View>(
    _ title: LocalizedStringKey, headingID: String? = nil, @ViewBuilder content: () -> Content
  ) -> some View {
    VStack(alignment: .leading, spacing: 6) {
      Text(title)
        .font(.headline)
        .accessibilityAddTraits(.isHeader)
        .accessibilityIdentifier(headingID ?? "")
      content()
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: .rect(cornerRadius: 8))
        .overlay {
          RoundedRectangle(cornerRadius: 8).strokeBorder(.quaternary)
            .allowsHitTesting(false)
        }
    }
  }

  @ViewBuilder private var storageSummary: some View {
    if let capacity = store.storagePlan?.capacity {
      VStack(alignment: .leading, spacing: 3) {
        Text(
          "Inference storage: \(bytes(capacity.usedBytes)) used of \(bytes(capacity.limitBytes))"
        )
        .font(.callout.weight(.medium))
        Text(
          "Reserved: \(bytes(capacity.reservedBytes)) · Available: \(bytes(capacity.availableBytes))"
        )
        .font(.caption).foregroundStyle(.secondary)
      }
    } else if let error = store.storageFeedback {
      Label("Storage estimate unavailable: \(error)", systemImage: "exclamationmark.circle")
        .font(.caption).foregroundStyle(.secondary)
    }
  }

  private func bytes(_ count: Int64) -> String {
    count.formatted(.byteCount(style: .file).locale(locale))
  }

  @ViewBuilder private var runtimeControls: some View {
    if backendReady, let runtime = store.runtime, runtime.state != .unavailable {
      librarySection("Local Runtime") {
        VStack(alignment: .leading, spacing: 8) {
          HStack {
            if let engine = runtime.engine {
              HStack(spacing: 8) {
                Image(systemName: "cpu").accessibilityHidden(true)
                Text(verbatim: engine.name)
                  .accessibilityIdentifier("runtime-engine-name")
              }
            } else {
              Label("Inference engine", systemImage: "cpu")
            }
            Spacer()
            if store.runtimeRequestPending || runtime.state == .installing {
              ProgressView().controlSize(.small)
              Text("Installing…").foregroundStyle(.secondary)
            } else if runtime.state == .ready {
              if let engine = runtime.engine {
                Text("Build \(engine.build)")
                  .foregroundStyle(.secondary)
                  .monospacedDigit()
                  .accessibilityIdentifier("runtime-engine-build")
              } else {
                Text("Ready").foregroundStyle(.secondary)
              }
            } else {
              if runtime.canDownload == true {
                Button("Download Runtime") { store.send(.downloadRuntime) }
                  .buttonStyle(.borderedProminent)
              }
              if runtime.archiveBytes != nil {
                Button("Choose ZIP…", action: chooseRuntimeArchive)
              }
            }
          }
          if runtime.state != .ready, runtime.canDownload == true {
            Text("Source: GitHub Releases").font(.caption).foregroundStyle(.secondary)
          }
          if runtime.state == .availableForInstall || runtime.state == .failed,
            let plan = store.storagePlan?.runtime
          {
            if runtime.archiveBytes != nil {
              Text(
                "ZIP import peak: \(bytes(plan.localImport.peakBytes))"
                  + (plan.localImport.fits ? "" : " · Insufficient available space")
              )
              .font(.caption).foregroundStyle(.secondary)
            }
            if runtime.canDownload == true, let hosted = plan.hostedDownload {
              Text(
                "Download peak: \(bytes(hosted.peakBytes))"
                  + (hosted.fits ? "" : " · Insufficient available space")
              )
              .font(.caption).foregroundStyle(.secondary)
            }
          }
          if let archiveBytes = runtime.archiveBytes {
            Text("Runtime archive: \(bytes(archiveBytes))")
              .font(.caption).foregroundStyle(.secondary)
          }
          if let error = store.runtimeFeedback ?? runtime.error {
            Text(error).font(.caption).foregroundStyle(.secondary)
          }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
      }
    } else if backendReady, let error = store.runtimeStatusFeedback {
      Label(error, systemImage: "exclamationmark.circle")
        .font(.caption).foregroundStyle(.secondary)
    }
  }

  @ViewBuilder private var indexControls: some View {
    if let index = store.index {
      librarySection("Embedding Index") {
        VStack(alignment: .leading, spacing: 10) {
          HStack {
            Text("Semantic indexing").accessibilityHidden(true)
            Spacer()
            Toggle(
              "Semantic indexing",
              isOn: Binding(
                get: { index.enabled },
                set: { store.send(.setIndexEnabled($0)) }
              )
            )
            .toggleStyle(.switch)
            .labelsHidden()
            .accessibilityIdentifier("embedding-index-enabled")
            .disabled(store.indexPending)
          }
          Divider()
          HStack {
            Text("Stored embeddings")
            Spacer()
            Button("Delete Index…", role: .destructive) { confirmingIndexDeletion = true }
              .accessibilityIdentifier("embedding-index-delete")
              .disabled(store.indexPending)
          }
          if let removal = store.indexRemoval {
            let size = bytes(removal.value2.removedBytes)
            Text(
              "Removed \(removal.value2.removedVectors) vectors (\(size) of logical data). Disk space reclaimed may differ."
            )
            .font(.caption).foregroundStyle(.secondary)
          } else if let feedback = store.indexFeedback ?? store.indexStatusFeedback {
            Text(feedback).font(.caption).foregroundStyle(.secondary)
          }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
      }
    } else if let feedback = store.indexStatusFeedback ?? store.indexFeedback {
      Label(feedback, systemImage: "exclamationmark.circle")
        .font(.caption).foregroundStyle(.secondary)
    }
  }

  private func chooseRuntimeArchive() {
    let panel = NSOpenPanel()
    panel.allowedContentTypes = [.zip]
    panel.allowsMultipleSelection = false
    panel.canChooseDirectories = false
    panel.message = locale.localized("Choose the trusted runtime release ZIP.")
    if panel.runModal() == .OK, let url = panel.url {
      store.send(.installRuntime(url.path))
    }
  }

  private var transferActive: Bool {
    store.setup.inProgress || store.pendingProfile != nil
      || store.runtimeRequestPending || store.runtime?.state == .installing
      || store.profiles?.contains(where: { $0.state == .installing || $0.state == .pausing })
        == true
  }

  private func showModelFolder() {
    do {
      try FileManager.default.createDirectory(
        at: inferenceDirectory, withIntermediateDirectories: true)
      modelFolderError = !NSWorkspace.shared.open(inferenceDirectory)
    } catch {
      modelFolderError = true
    }
  }
}

extension Components.Schemas.ModelDownload {
  fileprivate var isDownload: Bool {
    switch state {
    case .installing, .pausing, .paused, .interrupted, .failed: true
    case .notInstalled, .installed: false
    }
  }
}
