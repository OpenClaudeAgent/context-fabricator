import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct GitHubView: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<GitHubConnector>
  let traceInspector: StoreOf<TraceInspector>
  let answers: StoreOf<Answers>
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
      GitHubSheetContent(store: store, traceInspector: traceInspector, answers: answers)
        .interactiveDismissDisabled(store.busy)
        .safeAreaInset(edge: .bottom, spacing: 0) {
          if store.collectionReadFailure != nil {
            collectionStatusWarning
              .padding(.horizontal, 22)
              .padding(.vertical, 12)
              .background(.bar)
          }
        }
    }
    .onAppear { store.send(.workspaceVisibilityChanged(true)) }
    .onDisappear { store.send(.workspaceVisibilityChanged(false)) }
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
          LoadingStateView("Loading GitHub…")
        }
        if store.panel == nil {
          if let message = store.feedback { GitHubFeedback(message: message) }
          collectionStatusWarning
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

  @ViewBuilder private var collectionStatusWarning: some View {
    if let reason = store.collectionReadFailure {
      HStack(spacing: 12) {
        Label(
          "Couldn’t refresh collection status. Showing the last known status.",
          systemImage: "exclamationmark.circle"
        )
        .font(.callout)
        .foregroundStyle(.secondary)
        .help(reason)
        Spacer()
        Button("Reload Status") { store.send(.reload) }
          .disabled(store.busy)
      }
      .accessibilityIdentifier("github-collection-status-warning")
    }
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
        Button("Requests & Storage…", systemImage: "gauge.with.dots.needle.50percent") {
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
        Label(
          snapshot.login ?? locale.localized("Connected"), systemImage: "checkmark.circle.fill"
        )
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

  @ViewBuilder private func repositoryList(
    _ configured: [Components.Schemas.GitHubConfiguredRepository]
  )
    -> some View
  {
    let matches = configured.filter {
      filter.isEmpty || $0.repository.fullName.localizedCaseInsensitiveContains(filter)
    }
    if matches.isEmpty {
      ContentUnavailableView.search(text: filter)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    } else {
      List(matches, id: \.repository.id) { repository in
        repositoryRow(repository)
          .listRowInsets(EdgeInsets())
          .listRowBackground(Rectangle().fill(.quaternary.opacity(0.35)))
          .listRowSeparator(.visible)
      }
      .listStyle(.inset)
      .scrollContentBackground(.hidden)
    }
  }

  private func repositoryRow(_ repository: Components.Schemas.GitHubConfiguredRepository)
    -> some View
  {
    Button {
      store.send(.edit(repository))
    } label: {
      HStack(spacing: 12) {
        Image(systemName: repository.repository._private ? "lock.fill" : "book.closed")
          .font(.system(size: 18, weight: .medium))
          .foregroundStyle(.secondary)
          .frame(width: 36, height: 36)
          .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 8))
          .accessibilityHidden(true)
        VStack(alignment: .leading, spacing: 4) {
          Text(repository.repository.fullName).font(.body.weight(.medium)).lineLimit(2)
          Label(repository.branch, systemImage: "arrow.triangle.branch")
            .font(.caption)
            .foregroundStyle(.secondary)
          if let collection = repository.codeCollection {
            GitHubCollectionListStatus(status: collection)
          }
        }
        .labelStyle(GitHubRepositoryMetadataStyle())
        Spacer(minLength: 12)
        HStack(alignment: .firstTextBaseline, spacing: 8) {
          if let warning = repository.accessWarning(in: locale) {
            Label(warning, systemImage: "exclamationmark.circle")
              .font(.caption)
              .foregroundStyle(.orange)
              .fixedSize(horizontal: false, vertical: true)
              .accessibilityHidden(true)
          }
          Image(systemName: "chevron.right")
            .font(.caption.weight(.semibold))
            .foregroundStyle(.secondary)
            .frame(width: 12)
            .accessibilityHidden(true)
        }
      }
      .padding(.horizontal, 14)
      .padding(.vertical, 12)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .disabled(store.busy)
    .accessibilityValue(repository.accessWarning(in: locale) ?? "")
    .accessibilityIdentifier("github-edit-\(repository.repository.id)")
  }
}

private struct GitHubRepositoryMetadataStyle: LabelStyle {
  func makeBody(configuration: Configuration) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 6) {
      configuration.icon.frame(width: 12)
      configuration.title
    }
  }
}

extension Components.Schemas.GitHubConfiguredRepository {
  fileprivate func accessWarning(in locale: Locale) -> String? {
    if !verified { return locale.localized("Verify access") }
    if actions == .unavailable && contents == .unavailable {
      return locale.localized("Access unverified")
    }
    if actions == .unavailable { return locale.localized("Actions access unverified") }
    if contents == .unavailable { return locale.localized("Contents access unverified") }
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
  @Environment(\.locale) private var locale
  let message: String

  var body: some View {
    Label(locale.localizedKey(message), systemImage: "exclamationmark.circle")
      .font(.callout)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .accessibilityIdentifier("github-feedback")
  }
}
