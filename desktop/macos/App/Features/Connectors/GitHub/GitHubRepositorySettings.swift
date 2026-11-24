import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct GitHubOptionsSheet: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<GitHubConnector>
  let draft: GitHubConnector.Draft
  let showTrace: (Components.Schemas.GitHubCollectionTrace) -> Void
  @State private var remove = false
  @State private var purge = false

  private var configured: Components.Schemas.GitHubConfiguredRepository? {
    guard store.panel == .editOptions else { return nil }
    return store.snapshot?.repositories.first { $0.repository.id == draft.expectedId }
  }

  private var hasChanges: Bool {
    guard let configured else { return true }
    return draft.branch != configured.branch || draft.intervalMinutes != configured.intervalMinutes
      || draft.features != configured.features
  }

  private var feedback: String? {
    guard store.requiresDraftReview else { return store.feedback }
    if !store.canReviewRepositoryEdits {
      return locale.localized("This repository was removed. Close settings to add it again.")
    }
    if store.panel == .addOptions {
      return locale.localized("GitHub settings changed. Review your edits before saving.")
    }
    return
      "Repository settings changed. Review your edits with the latest settings, or use the latest settings."
  }

  var body: some View {
    GitHubSheet(
      title: store.panel == .editOptions ? "Repository Settings" : "Configure Repository",
      subtitle: draft.repository, message: feedback, busy: store.busy,
      width: configured == nil ? .standard : .wide, prefersTallContent: configured != nil
    ) {
      if let configured {
        repositorySettings(configured)
      } else {
        Form {
          repositorySection
          sourcesSection
        }
        .formStyle(.grouped)
        .contentMargins(.bottom, 16, for: .scrollContent)
        .frame(minHeight: 300, idealHeight: 370, maxHeight: 450)
      }
    } buttons: {
      footer
    }
    .task(id: configured?.repository.id) {
      if let configured { await store.send(.loadDataState(configured.repository.id)).finish() }
    }
    .onExitCommand { store.send(.dismissPanel) }
    .alert("Delete local repository data?", isPresented: $purge) {
      Button("Cancel", role: .cancel) {}
      Button("Delete Local Data", role: .destructive) {
        if let id = configured?.repository.id { store.send(.purgeData(id)) }
      }
    } message: {
      Text(
        "This deletes all retained local code, history, Actions and graph data for this repository, including its other branches. Settings remain. Collection pauses until you explicitly refresh an enabled, verified feature."
      )
    }
    .alert("Remove this repository?", isPresented: $remove) {
      Button("Cancel", role: .cancel) {}
      Button("Remove Repository", role: .destructive) {
        if let id = draft.expectedId { store.send(.remove(id)) }
      }
    } message: {
      Text("This removes the repository settings from Context Fabricator.")
    }
  }

  private func repositorySettings(_ repository: Components.Schemas.GitHubConfiguredRepository)
    -> some View
  {
    HSplitView {
      Form {
        repositorySection
        sourcesSection
      }
      .frame(minWidth: 360, idealWidth: 430)

      Form {
        accessSection(repository)
        if repository.features.actions {
          actionsCollection(repository)
        }
        if let status = repository.codeCollection {
          GitHubCollectionSection(
            status: status,
            title: repository.features.codeGraph && !repository.features.currentCode
              ? "Code graph" : "Branch files",
            canRefresh: canRefresh(repository, status: status),
            refresh: { store.send(.refreshCode(repository.repository.id)) }
          )
        }
        if repository.features.history {
          historyCollection(repository)
        }
        localDataSection(repository)
      }
      .accessibilityIdentifier("github-settings-status-form")
      .frame(minWidth: 370, idealWidth: 450)
    }
    .formStyle(.grouped)
    .contentMargins(.bottom, 16, for: .scrollContent)
  }

  private var repositorySection: some View {
    Section {
      if configured != nil {
        LabeledContent("Branch") {
          HStack(spacing: 6) {
            branchField
              .labelsHidden()
              .textFieldStyle(.plain)
              .multilineTextAlignment(.trailing)
            GitHubBranchPickerView(store: store, selection: draft.branch)
          }
        }
      } else {
        branchField
      }
      LabeledContent("Refresh interval") {
        HStack(spacing: 6) {
          TextField("Refresh interval", text: binding(\.intervalInput))
            .labelsHidden()
            .textFieldStyle(.roundedBorder)
            .frame(width: 70)
            .multilineTextAlignment(.trailing)
            .accessibilityIdentifier("github-interval")
          Text("min").foregroundStyle(.secondary)
        }
      }
    } header: {
      Text("Repository")
    } footer: {
      if let range = store.snapshot?.defaults.refreshInterval {
        Text(
          "\(range.minimumMinutes.formatted(.number.grouping(.never))) to \(range.maximumMinutes.formatted(.number.grouping(.never))) minutes"
        )
        .foregroundStyle(store.canSaveRefreshInterval ? Color.secondary : Color.orange)
        .accessibilityIdentifier("github-interval-range")
      } else if !store.canSaveRefreshInterval {
        Text("Enter the interval in minutes.")
          .foregroundStyle(.orange)
          .accessibilityIdentifier("github-interval-range")
      }
    }
  }

  private var branchField: some View {
    TextField("Branch", text: binding(\.branch), prompt: Text("Default branch"))
      .accessibilityIdentifier("github-branch")
  }

  private var sourcesSection: some View {
    Section("Sources") {
      option(branchSourceLabel, id: "github-current-code", field: \.features.currentCode)
        .help("Index files from the latest collected commit on this branch.")
      option("Commit history", id: "github-history", field: \.features.history)
        .help("Collect past commits and changes on this branch.")
      option("GitHub Actions", id: "github-actions", field: \.features.actions)
      option("Code graph", id: "github-code-graph", field: \.features.codeGraph)
        .help("Index symbols and their relationships in supported source files.")
      if let configured, configured.features.codeGraph {
        LabeledContent("Indexed symbols") {
          Button("Browse Code Graph…") {
            store.send(.openGraph(id: configured.repository.id, branch: configured.branch))
          }
          .accessibilityIdentifier("github-browse-graph")
        }
      }
    }
  }

  private var branchSourceLabel: LocalizedStringKey {
    let branch = (store.draft ?? draft).branch.trimmingCharacters(in: .whitespacesAndNewlines)
    return branch.isEmpty ? "Branch" : "Branch · \(branch)"
  }

  private func actionsCollection(_ repository: Components.Schemas.GitHubConfiguredRepository)
    -> some View
  {
    let schedule =
      store.actionsRepositoryId == repository.repository.id ? store.actionsState?.schedule : nil
    return Section {
      LabeledContent("Workflow runs") {
        Button("View Workflow Runs…") { store.send(.openActions(repository.repository.id)) }
          .accessibilityIdentifier("github-view-actions")
      }
      LabeledContent("Workflow collection") {
        HStack(spacing: 8) {
          GitHubCollectionTraceLinks(
            activity: repository.actionsActivity, feature: "actions", showTrace: showTrace
          )
          .labelStyle(.iconOnly)
          Button("Refresh Now", systemImage: "arrow.clockwise") { store.send(.refreshActions) }
            .controlSize(.small)
            .accessibilityIdentifier("github-refresh-repository-actions")
        }
      }
      if let schedule {
        LabeledContent("Next check") {
          if schedule.nextAtMs == 0 {
            Text("Due now")
          } else {
            Text(
              Date(timeIntervalSince1970: Double(schedule.nextAtMs) / 1_000),
              format: .dateTime.month().day().hour().minute()
            )
          }
        }
        .accessibilityIdentifier("github-actions-next-check")
      }
    } header: {
      Text("GitHub Actions")
    } footer: {
      if let error = schedule?.lastError {
        Label(actionsScheduleError(error), systemImage: "exclamationmark.circle")
          .foregroundStyle(.orange)
      }
    }
  }

  private func actionsScheduleError(
    _ value: Components.Schemas.GitHubActionsState.schedulePayload.lastErrorPayload
  ) -> LocalizedStringKey {
    switch value {
    case .throttled: "Waiting for GitHub rate limit"
    case .credentials: "GitHub credentials unavailable"
    case .access: "Repository access required"
    case .staleScope: "Repository settings changed"
    case .collectionLimit: "Collection limit reached"
    case .storageLimit:
      "Local storage limit reached. Increase the GitHub storage limit or remove collected data."
    case .unavailable: "GitHub is unavailable"
    }
  }

  private func historyCollection(_ repository: Components.Schemas.GitHubConfiguredRepository)
    -> some View
  {
    Section("Commit History Collection") {
      LabeledContent("Commits") {
        HStack(spacing: 8) {
          GitHubCollectionTraceLinks(
            activity: repository.historyActivity, feature: "history", showTrace: showTrace
          )
          .labelStyle(.iconOnly)
          Button("Refresh History") { store.send(.refreshHistory(repository.repository.id)) }
            .controlSize(.small)
            .disabled(
              !repository.verified || repository.contents != .available
                || store.snapshot?.credential != .connected
            )
            .accessibilityIdentifier("github-refresh-history")
        }
      }
    }
  }

  private func localDataSection(_ repository: Components.Schemas.GitHubConfiguredRepository)
    -> some View
  {
    Section {
      if store.dataStateId == repository.repository.id, store.dataPaused == true {
        VStack(alignment: .leading, spacing: 4) {
          Label("Automatic collection paused", systemImage: "pause.circle")
            .foregroundStyle(.orange)
          Text("Local data was deleted. Refresh an enabled source to resume collection.")
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Automatic collection paused")
        .accessibilityHint(
          "Local data was deleted. Refresh an enabled source to resume collection."
        )
        .accessibilityIdentifier("github-data-pause-state")
      }
      LabeledContent("Data on this Mac") {
        Button("Delete Local Data…", role: .destructive) { purge = true }
          .accessibilityIdentifier("github-purge-data")
      }
      LabeledContent("Configured repository") {
        Button("Remove Repository…", role: .destructive) { remove = true }
          .accessibilityIdentifier("github-remove")
      }
    } header: {
      Text("Local Repository Data")
    }
  }

  @ViewBuilder private var footer: some View {
    if store.panel == .addOptions {
      Button("Back") { store.send(.backToPicker) }
        .disabled(store.busy)
        .accessibilityIdentifier("github-picker-back")
    }
    Spacer()
    if store.requiresDraftReview {
      conflictButtons
    } else if hasChanges {
      if store.panel == .editOptions {
        Button("Cancel", role: .cancel) { store.send(.dismissPanel) }
          .disabled(store.busy)
          .keyboardShortcut(.cancelAction)
      }
      Button("Verify & Save") { store.send(.save) }
        .buttonStyle(.borderedProminent)
        .keyboardShortcut(.defaultAction)
        .disabled(
          store.busy || store.snapshot?.credential != .connected || !store.canSaveRefreshInterval
        )
        .accessibilityIdentifier("github-save")
    } else {
      Button("Done") { store.send(.dismissPanel) }
        .keyboardShortcut(.defaultAction)
        .disabled(store.busy)
        .accessibilityIdentifier("github-settings-done")
    }
  }

  @ViewBuilder private var conflictButtons: some View {
    if store.panel == .editOptions {
      Button("Cancel", role: .cancel) { store.send(.dismissPanel) }
        .keyboardShortcut(.cancelAction)
        .disabled(store.busy)
      if store.canReviewRepositoryEdits {
        Button("Use Latest Settings") { store.send(.resolveDraft(.useLatest)) }
          .disabled(store.busy)
          .accessibilityIdentifier("github-draft-use-latest")
      }
    }
    if store.canReviewRepositoryEdits {
      Button("Review My Edits") { store.send(.resolveDraft(.keepEdits)) }
        .buttonStyle(.borderedProminent)
        .disabled(store.busy)
        .accessibilityIdentifier("github-draft-review")
    }
  }

  private func canRefresh(
    _ repository: Components.Schemas.GitHubConfiguredRepository,
    status: Components.Schemas.GitHubCodeCollection
  ) -> Bool {
    status.canRefresh
      || (store.dataPaused == true
        && (repository.features.currentCode || repository.features.codeGraph)
        && repository.contents == .available && store.snapshot?.credential == .connected)
  }

  private func option(
    _ label: LocalizedStringKey, id: String, field: WritableKeyPath<GitHubConnector.Draft, Bool>
  ) -> some View {
    Toggle(label, isOn: binding(field))
      .toggleStyle(.switch)
      .accessibilityLabel(Text(label))
      .accessibilityIdentifier(id)
  }

  private func accessSection(_ repository: Components.Schemas.GitHubConfiguredRepository)
    -> some View
  {
    let connected = store.snapshot?.credential == .connected
    return Section {
      accessRow("Contents", status: repository.contents, id: "github-contents-access")
      accessRow("GitHub Actions", status: repository.actions, id: "github-actions-access")
      LabeledContent("Credentials") {
        Button(connected ? "Manage Token…" : "Reconnect GitHub…") {
          store.send(.openRepositoryAccount)
        }
        .accessibilityIdentifier("github-repository-account")
      }
      if connected {
        LabeledContent("Verification") {
          Button("Check Access") { store.send(.verifyAccess) }
            .accessibilityIdentifier("github-verify-access")
        }
      }
    } header: {
      Text("Repository Access")
    } footer: {
      if !connected {
        Text("Reconnect to check this repository using a GitHub token.")
      } else if repository.contents == .unavailable || repository.actions == .unavailable {
        Text(
          "Check the repository and branch, and confirm the token grants read access to the requested features."
        )
      }
    }
  }

  private func accessRow(_ title: String, status: Components.Schemas.GitHubAccess, id: String)
    -> some View
  {
    let label = accessLabel(status)
    return LabeledContent(title, value: label)
      .accessibilityElement(children: .combine)
      .accessibilityLabel("\(title), \(label)")
      .accessibilityIdentifier(id)
  }

  private func accessLabel(_ access: Components.Schemas.GitHubAccess) -> String {
    switch access {
    case .available: locale.localized("Read access verified")
    case .unavailable: locale.localized("Access unverified")
    case .unchecked: locale.localized("Awaiting verification")
    }
  }

  private func binding<T>(_ field: WritableKeyPath<GitHubConnector.Draft, T>) -> Binding<T> {
    Binding(
      get: { (store.draft ?? draft)[keyPath: field] },
      set: { value in
        var updated = store.draft ?? draft
        updated[keyPath: field] = value
        store.send(.change(updated))
      })
  }
}
