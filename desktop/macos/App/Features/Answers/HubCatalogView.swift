import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct HubCatalogView: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<HubCatalog>
  @State private var query = ""

  var body: some View {
    VStack(spacing: 0) {
      header
      content
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.horizontal, 24)
        .padding(.bottom, 18)
      Divider()
      footer
    }
  }

  private var header: some View {
    VStack(spacing: 12) {
      HStack {
        Text("Search Models").font(.title2.weight(.semibold)).accessibilityAddTraits(.isHeader)
        Spacer()
        Label("Hugging Face", systemImage: "globe")
          .font(.callout).foregroundStyle(.secondary)
      }
      Picker(
        "Model role",
        selection: Binding(
          get: { store.kind }, set: { store.send(.selectKind($0)) }
        )
      ) {
        Text("Answer Models").tag(HubModelKind.generation)
        Text("Embedding Models").tag(HubModelKind.embedding)
      }
      .pickerStyle(.segmented)
      .disabled(store.operation == .addingDownload)
      .accessibilityIdentifier("hugging-face-role")
      Group {
        if let repository = store.repository {
          repositoryHeader(repository)
        } else {
          searchField
        }
      }
      .frame(height: 32)
    }
    .padding(.horizontal, 24)
    .padding(.top, 22)
    .padding(.bottom, 14)
  }

  private var searchField: some View {
    HStack(spacing: 8) {
      TextField("Model name or publisher", text: $query)
        .textFieldStyle(.roundedBorder)
        .onSubmit(search)
        .accessibilityIdentifier("hugging-face-query")
      Button("Search", action: search)
        .disabled(store.operation != nil)
        .accessibilityIdentifier("hugging-face-search")
    }
  }

  private func search() {
    store.send(.search(query))
  }

  private func repositoryHeader(_ repository: HubCatalog.Repository) -> some View {
    HStack {
      VStack(alignment: .leading, spacing: 3) {
        Text("Compatible files").font(.headline)
        if let url = URL(string: repository.url) {
          Link(repository.repository, destination: url)
            .font(.callout)
            .lineLimit(1)
            .truncationMode(.middle)
        }
      }
      Spacer(minLength: 0)
    }
  }

  @ViewBuilder private var content: some View {
    if let operation = store.operation {
      loading(operation)
    } else if store.repository != nil, let variants = store.variants {
      variantResults(variants)
    } else if let feedback = store.feedback {
      placeholder(
        "Couldn’t Load Models", description: Text(feedback), icon: "exclamationmark.circle")
    } else if let repositories = store.repositories {
      repositoryResults(repositories)
    } else {
      placeholder(
        "Search Hugging Face",
        description: store.kind == .embedding
          ? Text("Find public Qwen3-Embedding-0.6B GGUF models compatible with semantic search.")
          : Text("Find public Qwen3 4B Instruct models compatible with local Answers."),
        icon: "magnifyingglass")
    }
  }

  private func loading(_ operation: HubCatalog.Operation) -> some View {
    let status: LocalizedStringKey =
      switch operation {
      case .searching: "Searching Hugging Face…"
      case .loadingVariants: "Checking compatible files…"
      case .addingDownload: "Preparing verified download…"
      }
    return LoadingStateView(status).accessibilityIdentifier("hugging-face-loading")
  }

  @ViewBuilder private func repositoryResults(_ repositories: [HubCatalog.Repository]) -> some View
  {
    if repositories.isEmpty {
      placeholder(
        "No Models Found",
        description: store.kind == .embedding
          ? Text("Try another name or publisher for a public Qwen3-Embedding-0.6B GGUF model.")
          : Text("Try another model name or publisher."),
        icon: "magnifyingglass")
    } else {
      results {
        ForEach(repositories, id: \.repository) { repository in
          if repository != repositories.first { rowDivider }
          Button {
            store.send(.openRepository(repository))
          } label: {
            row(repository.repository, detail: nil, icon: "chevron.right")
          }
          .buttonStyle(.plain)
          .accessibilityIdentifier("hugging-face-repository-\(repository.repository)")
        }
      }
    }
  }

  @ViewBuilder private func variantResults(_ variants: [HubCatalog.Variant]) -> some View {
    if variants.isEmpty {
      placeholder(
        "No Compatible Files",
        description: Text("Return to search results and choose another model."),
        icon: "doc.badge.ellipsis")
    } else {
      results {
        ForEach(variants, id: \.file) { variant in
          if variant != variants.first { rowDivider }
          Button {
            store.send(.selectVariant(variant))
          } label: {
            row(
              variant.file,
              detail: store.kind == .embedding && variant.sha256?.isEmpty != false
                ? String(localized: "Verification unavailable")
                : variant.bytes.formatted(.byteCount(style: .file).locale(locale)),
              icon: store.selectedVariant == variant ? "checkmark.circle.fill" : "circle")
          }
          .buttonStyle(.plain)
          .disabled(store.kind == .embedding && variant.sha256?.isEmpty != false)
          .accessibilityAddTraits(store.selectedVariant == variant ? .isSelected : [])
          .accessibilityIdentifier("hugging-face-file-\(variant.file)")
        }
      }
    }
  }

  private func placeholder(_ title: LocalizedStringKey, description: Text, icon: String)
    -> some View
  {
    VStack(spacing: 8) {
      Image(systemName: icon).font(.title2).foregroundStyle(.secondary)
        .accessibilityHidden(true)
      Text(title).font(.headline)
      description.font(.callout).foregroundStyle(.secondary)
    }
    .multilineTextAlignment(.center)
    .padding(24)
  }

  private var footer: some View {
    HStack(spacing: 10) {
      if store.repository != nil {
        Button("Back", systemImage: "chevron.left") { store.send(.back) }
          .disabled(store.operation == .addingDownload)
      }
      if store.variants != nil, let feedback = store.feedback {
        Label(feedback, systemImage: "exclamationmark.circle")
          .font(.callout).foregroundStyle(.secondary)
          .lineLimit(1).help(feedback)
      }
      Spacer()
      Button("Done") { store.send(.close) }
        .keyboardShortcut(.cancelAction)
        .disabled(store.operation == .addingDownload)
      if store.repository != nil {
        Button("Download", systemImage: "arrow.down") { store.send(.download) }
          .buttonStyle(.borderedProminent)
          .disabled(store.selectedVariant == nil || store.operation != nil)
          .accessibilityIdentifier("hugging-face-download")
      }
    }
    .padding(.horizontal, 24)
    .padding(.vertical, 13)
    .background(.bar)
  }

  private func results(@ViewBuilder content: () -> some View) -> some View {
    ScrollView {
      LazyVStack(alignment: .leading, spacing: 0, content: content)
        .background(.background, in: .rect(cornerRadius: 8))
        .overlay {
          RoundedRectangle(cornerRadius: 8).strokeBorder(.quaternary).allowsHitTesting(false)
        }
    }
    .defaultScrollAnchor(.top)
  }

  private var rowDivider: some View {
    Divider().padding(.horizontal, 12)
  }

  private func row(_ title: String, detail: String?, icon: String) -> some View {
    HStack(spacing: 10) {
      VStack(alignment: .leading, spacing: 4) {
        Text(title).lineLimit(1).truncationMode(.middle)
        if let detail { Text(detail).font(.caption).foregroundStyle(.secondary) }
      }
      Spacer(minLength: 0)
      Image(systemName: icon)
        .foregroundStyle(.secondary)
        .accessibilityHidden(true)
    }
    .padding(.horizontal, 12)
    .padding(.vertical, 9)
    .contentShape(.rect)
  }
}
