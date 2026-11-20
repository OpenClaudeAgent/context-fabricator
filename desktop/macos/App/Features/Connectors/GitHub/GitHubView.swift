import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct GitHubView: View {
  let store: StoreOf<GitHubConnector>
  @State private var filter = ""
  @State private var searching = false

  var body: some View {
    Group {
      if store.managing {
        searchableManagement
      } else {
        directory
      }
    }
    .sheet(
      isPresented: Binding(
        get: { store.panel != nil },
        set: { if !$0 { store.send(.dismissPanel) } })
    ) {
      GitHubSheetContent(store: store)
        .interactiveDismissDisabled(store.busy)
    }
    .task(id: monitorsCollection) {
      guard monitorsCollection else { return }
      while !Task.isCancelled {
        try? await Task.sleep(for: .seconds(2))
        if Task.isCancelled { return }
        await store.send(.pollCollection).finish()
      }
    }
  }

  private var monitorsCollection: Bool {
    store.managing && store.snapshot?.repositories.isEmpty == false
  }

  private var directory: some View {
    WorkspacePage {
      WorkspaceHeading(title: "Connectors")
      Button {
        store.send(.manage)
      } label: {
        HStack(spacing: 14) {
          GitHubIcon(size: 36)
          VStack(alignment: .leading, spacing: 3) {
            Text("GitHub").font(.headline)
            Text("Repository access").foregroundStyle(.secondary)
          }
          Spacer()
          Image(systemName: "chevron.right")
            .font(.caption.weight(.semibold))
            .foregroundStyle(.tertiary)
        }
        .padding(14)
        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 12))
      }
      .buttonStyle(.plain)
      .accessibilityIdentifier("github-manage")
    }
  }

  @ViewBuilder
  private var searchableManagement: some View {
    if let snapshot = store.snapshot, !snapshot.repositories.isEmpty {
      management.searchable(
        text: $filter, isPresented: $searching, placement: .toolbar,
        prompt: "Filter configured repositories")
    } else {
      management
    }
  }

  private var management: some View {
    WorkspaceColumn {
      VStack(alignment: .leading, spacing: 20) {
        if let snapshot = store.snapshot {
          providerHeader(snapshot)
          repositorySection(snapshot)
        } else if store.busy {
          ProgressView("Loading GitHub…").frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        if store.panel == nil, let message = store.feedback {
          GitHubFeedback(message: message)
        }
      }
      .frame(maxHeight: .infinity, alignment: .topLeading)
      .padding(.bottom, 20)
    }
    .toolbar {
      ToolbarItem(placement: .navigation) {
        Button("Connectors", systemImage: "chevron.left") { store.send(.close) }
          .help("Back to Connectors")
          .accessibilityIdentifier("github-back")
      }
      ToolbarItemGroup(placement: .primaryAction) {
        Button("Reload GitHub settings", systemImage: "arrow.clockwise") { store.send(.reload) }
          .disabled(store.busy)
          .help("Reload GitHub settings")
          .accessibilityIdentifier("github-reload")
        if let snapshot = store.snapshot, !snapshot.repositories.isEmpty {
          addRepositoryButton(snapshot)
        }
      }
    }
    .focusedSceneValue(
      \.githubWorkspace, GitHubWorkspaceActions(store: store, searching: $searching))
  }

  private func providerHeader(_ snapshot: Components.Schemas.GitHubState) -> some View {
    HStack(spacing: 14) {
      GitHubIcon(size: 36)
      VStack(alignment: .leading, spacing: 4) {
        Text("GitHub")
          .font(.title2.bold())
          .accessibilityAddTraits(.isHeader)
        connectionLabel(snapshot)
      }
      Spacer()
      if snapshot.traffic != nil {
        Button("Requests…", systemImage: "gauge.with.dots.needle.50percent") {
          store.send(.openTraffic)
        }
        .disabled(store.busy)
        .accessibilityIdentifier("github-requests")
      }
      if snapshot.credential != .unconfigured || !snapshot.repositories.isEmpty {
        Button(
          snapshot.credential == .connected ? "Account…" : "Connect…",
          systemImage: "person.crop.circle"
        ) {
          store.send(.openAccount)
        }
        .disabled(store.busy)
        .accessibilityIdentifier("github-account")
      }
    }
  }

  private func connectionLabel(_ snapshot: Components.Schemas.GitHubState) -> some View {
    Group {
      switch snapshot.credential {
      case .unconfigured:
        Label("Not connected", systemImage: "link")
      case .connected:
        Label(snapshot.login ?? "Connected", systemImage: "checkmark.circle.fill")
          .symbolRenderingMode(.palette)
          .foregroundStyle(.green, .secondary)
      case .reconnectRequired:
        Label("Reconnect required", systemImage: "exclamationmark.circle")
          .symbolRenderingMode(.palette)
          .foregroundStyle(.orange, .secondary)
      }
    }
    .font(.callout)
    .foregroundStyle(.secondary)
    .accessibilityIdentifier("github-credential")
  }

  @ViewBuilder
  private func repositorySection(_ snapshot: Components.Schemas.GitHubState) -> some View {
    if snapshot.repositories.isEmpty, snapshot.credential != .connected {
      ContentUnavailableView {
        Label("Connect to GitHub", systemImage: "link.badge.plus")
      } description: {
        Text("Choose the repositories Context Fabricator can read.")
      } actions: {
        Button("Connect to GitHub…") { store.send(.openAccount) }
          .buttonStyle(.borderedProminent)
          .accessibilityIdentifier("github-connect")
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    } else {
      VStack(alignment: .leading, spacing: 12) {
        if snapshot.repositories.isEmpty {
          ContentUnavailableView {
            Label("No repositories added", systemImage: "folder")
          } description: {
            Text("Add a repository to choose its branch and what to collect.")
          } actions: {
            addRepositoryButton(snapshot).buttonStyle(.borderedProminent)
          }
          .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
          Text("Repositories").font(.headline).accessibilityAddTraits(.isHeader)
          repositoryList(snapshot.repositories)
        }
      }
      .frame(maxHeight: .infinity, alignment: .topLeading)
    }
  }

  private func addRepositoryButton(_ snapshot: Components.Schemas.GitHubState) -> some View {
    Button("Add Repository…", systemImage: "plus") { store.send(.add) }
      .disabled(snapshot.credential != .connected || store.busy)
      .help("Add Repository")
      .accessibilityIdentifier("github-add")
  }

  private func repositoryList(_ configured: [Components.Schemas.GitHubConfiguredRepository])
    -> some View
  {
    let matches = configured.filter {
      filter.isEmpty || $0.repository.fullName.localizedCaseInsensitiveContains(filter)
    }
    return List(matches, id: \.repository.id) { repository in
      Button {
        store.send(.edit(repository))
      } label: {
        HStack(spacing: 12) {
          Image(systemName: repository.repository._private ? "lock.square" : "book.closed")
            .font(.title3)
            .foregroundStyle(.secondary)
            .frame(width: 26)
            .accessibilityHidden(true)
          VStack(alignment: .leading, spacing: 4) {
            Text(repository.repository.fullName).font(.body.weight(.medium)).lineLimit(2)
            Label(repository.branch, systemImage: "arrow.triangle.branch")
              .font(.caption)
              .foregroundStyle(.secondary)
            if let collection = repository.codeCollection {
              GitHubCollectionLabel(phase: collection.phase)
                .font(.caption)
                .foregroundStyle(.secondary)
            }
          }
          Spacer()
          if let warning = repository.accessWarning {
            Label(warning, systemImage: "exclamationmark.circle")
              .font(.caption)
              .foregroundStyle(.secondary)
              .fixedSize(horizontal: false, vertical: true)
              .accessibilityHidden(true)
          }
          Image(systemName: "chevron.right")
            .font(.caption.weight(.semibold))
            .foregroundStyle(.tertiary)
            .accessibilityHidden(true)
        }
        .padding(.vertical, 7)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .disabled(store.busy)
      .accessibilityValue(repository.accessWarning ?? "")
      .accessibilityIdentifier("github-edit-\(repository.repository.id)")
    }
    .listStyle(.inset)
    .overlay {
      if matches.isEmpty { ContentUnavailableView.search(text: filter) }
    }
  }
}

extension Components.Schemas.GitHubConfiguredRepository {
  fileprivate var accessWarning: String? {
    if !verified { return "Verify access" }
    if actions == .unavailable && contents == .unavailable { return "Access unverified" }
    if actions == .unavailable { return "Actions access unverified" }
    if contents == .unavailable { return "Contents access unverified" }
    return nil
  }
}

struct GitHubIcon: View {
  let size: CGFloat

  var body: some View {
    Image(systemName: "chevron.left.forwardslash.chevron.right")
      .font(.system(size: size * 0.44, weight: .semibold))
      .foregroundStyle(.primary)
      .frame(width: size, height: size)
      .background(.quaternary, in: RoundedRectangle(cornerRadius: size * 0.25))
      .accessibilityHidden(true)
  }
}

struct GitHubFeedback: View {
  let message: String

  var body: some View {
    Label(message, systemImage: "exclamationmark.circle")
      .font(.callout)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .accessibilityIdentifier("github-feedback")
  }
}
