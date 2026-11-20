import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct AnswerModelControls: View {
  let store: StoreOf<Answers>
  let connection: Dashboard.Connection
  let restart: () -> Void
  let showModelLibrary: () -> Void

  var body: some View {
    ViewThatFits(in: .horizontal) {
      HStack(spacing: 12) { controls }
      VStack(alignment: .leading, spacing: 8) { controls }
    }
  }

  @ViewBuilder private var controls: some View {
    if connection == .connecting {
      ProgressView("Connecting to the backend").controlSize(.small)
      libraryMenu
    } else if connection == .ready, let models = store.models, let active = store.activeModel {
      modelMenu(active, profiles: models.profiles)
    } else if connection == .ready {
      if store.models != nil {
        Button("Local model required", systemImage: "cpu", action: showModelLibrary)
          .buttonStyle(.borderless)
          .font(.callout.weight(.semibold))
          .help("Open the model library")
          .accessibilityIdentifier("manage-models")
      } else if store.modelFeedback == nil {
        ProgressView("Checking local models").controlSize(.small)
        libraryMenu
      } else {
        Button("Retry Model Check", systemImage: "arrow.clockwise") { store.send(.loadModels) }
        libraryMenu
      }
    } else {
      Button("Restart Backend", systemImage: "arrow.clockwise", action: restart)
        .disabled(store.answerBusy || store.modelSwitching)
      libraryMenu
    }
  }

  private func modelMenu(
    _ active: Components.Schemas.GenerationProfile, profiles: [Components.Schemas.GenerationProfile]
  ) -> some View {
    HStack(spacing: 12) {
      Menu {
        ForEach(profiles, id: \.id) { model in
          Button {
            store.send(.selectModel(model.id))
          } label: {
            if model.id == active.id {
              Label(model.label, systemImage: "checkmark")
            } else {
              Text(model.label)
            }
          }
          .disabled(store.answerBusy || store.modelSwitching)
        }
        Divider()
        manageModelsAction
      } label: {
        Label(active.label, systemImage: "cpu").font(.callout)
          .labelStyle(.titleAndIcon)
      }
      .menuStyle(.borderlessButton)
      .fixedSize()
      .help("Choose a local model or manage models")
      .accessibilityLabel("Local model, \(active.label)")
      .accessibilityIdentifier("generation-model-picker")

      if let loading = profiles.first(where: { $0.id == store.loadingModel }) {
        modelProgress("Loading \(loading.label)…", id: "generation-model-loading")
      } else if store.answerBusy {
        modelProgress("Generating locally", id: "answer-generation-progress")
      }
    }
  }

  private var manageModelsAction: some View {
    Button("Manage Models…", systemImage: "slider.horizontal.3", action: showModelLibrary)
      .accessibilityIdentifier("manage-models")
  }

  private var libraryMenu: some View {
    Menu {
      manageModelsAction
    } label: {
      Label("Models", systemImage: "cpu")
    }
    .menuStyle(.borderlessButton)
    .fixedSize()
    .help("Open the model library")
    .accessibilityIdentifier("generation-model-picker")
  }

  private func modelProgress(_ title: String, id: String) -> some View {
    HStack(spacing: 7) {
      ProgressView().controlSize(.small).accessibilityHidden(true)
      Text(title).font(.callout).foregroundStyle(.secondary).lineLimit(1)
        .accessibilityIdentifier(id)
    }
  }
}
