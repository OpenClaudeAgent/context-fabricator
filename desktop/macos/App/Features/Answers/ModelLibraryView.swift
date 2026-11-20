import AppKit
import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct ModelLibraryView: View {
  let store: StoreOf<ModelLibrary>
  let inferenceDirectory: URL
  let backendReady: Bool
  let restartBlocked: Bool
  let restart: () -> Void
  @Environment(\.dismiss) private var dismiss
  @State private var modelFolderError = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      Text("Manage Models")
        .font(.title2.weight(.semibold))
        .accessibilityAddTraits(.isHeader)
        .padding(.horizontal, 24)
        .padding(.top, 22)
        .padding(.bottom, 18)

      VStack(alignment: .leading, spacing: 14) {
        catalog
        Label {
          Text(
            "Restart the backend to activate downloaded models. A fresh setup also needs the llama-server executable in the model folder."
          )
        } icon: {
          Image(systemName: "info.circle").accessibilityHidden(true)
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      }
      .padding(.horizontal, 24)
      .padding(.bottom, 18)

      Divider()
      HStack(spacing: 8) {
        Button("Open Model Folder", systemImage: "folder", action: showModelFolder)
        Button("Restart Backend", systemImage: "arrow.clockwise", action: restart)
          .disabled(!backendReady || restartBlocked || transferActive || store.profiles == nil)
        Spacer()
        Button("Done") { dismiss() }
          .keyboardShortcut(.cancelAction)
          .buttonStyle(.borderedProminent)
      }
      .controlSize(.regular)
      .padding(.horizontal, 24)
      .padding(.vertical, 13)
      .background(.bar)
    }
    .frame(width: 565)
    .alert("Couldn’t Open the Model Folder", isPresented: $modelFolderError) {
      Button("OK", role: .cancel) {}
    }
    .task(id: backendReady) {
      if backendReady { await store.send(.observe).finish() }
    }
    .onDisappear { store.send(.stopObserving) }
  }

  @ViewBuilder private var catalog: some View {
    if !backendReady {
      Label("The local backend must be ready to manage downloads.", systemImage: "info.circle")
        .foregroundStyle(.secondary)
    } else if let profiles = store.profiles {
      modelSections(profiles)
    } else if store.catalogFeedback == nil {
      ProgressView("Loading the model library").controlSize(.small)
    }
    if backendReady, let catalogFeedback = store.catalogFeedback {
      Label(catalogFeedback, systemImage: "exclamationmark.circle")
        .foregroundStyle(.secondary)
    }
    if let feedback = store.feedback {
      Label(feedback, systemImage: "exclamationmark.circle")
        .foregroundStyle(.secondary)
        .accessibilityIdentifier("model-library-feedback")
    }
  }

  @ViewBuilder private func modelSections(_ models: [Components.Schemas.ModelDownload]) -> some View
  {
    modelSection("Answer models", models: models.filter { $0.kind == .generation })
    modelSection("Embedding models", models: models.filter { $0.kind == .embedding })
    modelSection("Available models", models: models.filter { $0.kind == nil })
  }

  @ViewBuilder private func modelSection(
    _ title: LocalizedStringKey, models: [Components.Schemas.ModelDownload]
  ) -> some View {
    if !models.isEmpty {
      VStack(alignment: .leading, spacing: 6) {
        Text(title).font(.callout.weight(.semibold)).accessibilityAddTraits(.isHeader)
        VStack(spacing: 0) {
          ForEach(models, id: \.profile) { model in
            if model.profile != models.first?.profile { Divider().padding(.horizontal, 12) }
            ModelDownloadRow(
              model: model,
              requesting: store.pendingProfile == model.profile,
              requestPending: store.pendingProfile != nil
            ) { store.send(.install(model.profile)) }
          }
        }
        .background(.background, in: .rect(cornerRadius: 8))
        .overlay {
          RoundedRectangle(cornerRadius: 8).strokeBorder(.quaternary)
            .allowsHitTesting(false)
        }
      }
    }
  }

  private var transferActive: Bool {
    store.pendingProfile != nil
      || store.profiles?.contains(where: { $0.state == .installing }) == true
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
