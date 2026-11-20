import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct GitHubOptionsSheet: View {
  let store: StoreOf<GitHubConnector>
  let draft: GitHubConnector.Draft
  @State private var remove = false

  private var configured: Components.Schemas.GitHubConfiguredRepository? {
    guard store.panel == .editOptions else { return nil }
    return store.snapshot?.repositories.first { $0.repository.id == draft.expectedId }
  }

  var body: some View {
    GitHubSheet(title: "Repository Settings", message: store.feedback, busy: store.busy) {
      Form {
        Section {
          LabeledContent("Repository") {
            Text(draft.repository)
              .multilineTextAlignment(.trailing)
              .fixedSize(horizontal: false, vertical: true)
          }
          TextField("Branch", text: binding(\.branch), prompt: Text("Default branch"))
            .accessibilityIdentifier("github-branch")
          LabeledContent("Refresh interval") {
            HStack {
              TextField("Refresh interval", value: binding(\.intervalMinutes), format: .number)
                .labelsHidden().frame(width: 60).multilineTextAlignment(.trailing)
                .accessibilityIdentifier("github-interval")
              Text("min").foregroundStyle(.secondary)
            }
          }
          if let status = configured?.codeCollection {
            GitHubCollectionSummary(status: status)
          }
        }
        if let configured { accessSection(configured) }
        if let status = configured?.codeCollection,
          status.publishedAtMs != nil || status.lastCheckedAtMs != nil || status.nextAtMs != nil
            || status.error != nil
        {
          GitHubCollectionSection(status: status)
        }
        Section("Include") {
          option("Current code", id: "github-current-code", field: \.features.currentCode)
          option("Code history", id: "github-history", field: \.features.history)
          option("GitHub Actions", id: "github-actions", field: \.features.actions)
          option("Code graph", id: "github-code-graph", field: \.features.codeGraph)
        }
      }
      .formStyle(.grouped)
      .contentMargins(.bottom, 16, for: .scrollContent)
      .frame(minHeight: 290, idealHeight: configured == nil ? 380 : 530, maxHeight: 580)
    } buttons: {
      if store.panel == .editOptions {
        Button("Remove…", role: .destructive) { remove = true }
          .disabled(store.busy)
          .accessibilityIdentifier("github-remove")
        if let configured, let status = configured.codeCollection {
          Button("Refresh Now", systemImage: "arrow.clockwise") {
            store.send(.refreshCode(configured.repository.id))
          }
          .disabled(store.busy || !status.canRefresh)
          .accessibilityIdentifier("github-refresh-code")
        }
      }
      Spacer()
      if store.panel == .addOptions {
        Button("Back") { store.send(.backToPicker) }
          .disabled(store.busy)
          .accessibilityIdentifier("github-picker-back")
      } else {
        Button("Cancel", role: .cancel) { store.send(.dismissPanel) }
          .disabled(store.busy)
          .keyboardShortcut(.cancelAction)
      }
      Button("Verify & Save") { store.send(.save) }
        .keyboardShortcut(.defaultAction)
        .disabled(store.busy || store.snapshot?.credential != .connected)
        .accessibilityIdentifier("github-save")
    }
    .onExitCommand { store.send(.dismissPanel) }
    .alert("Remove this repository?", isPresented: $remove) {
      Button("Cancel", role: .cancel) {}
      Button("Remove Repository", role: .destructive) {
        if let id = draft.expectedId { store.send(.remove(id)) }
      }
    } message: {
      Text("This removes the repository settings from Context Fabricator.")
    }
  }

  private func option(
    _ label: LocalizedStringKey, id: String, field: WritableKeyPath<GitHubConnector.Draft, Bool>
  ) -> some View {
    Toggle(label, isOn: binding(field))
      .toggleStyle(.switch)
      .accessibilityIdentifier(id)
  }

  private func accessSection(_ repository: Components.Schemas.GitHubConfiguredRepository)
    -> some View
  {
    let connected = store.snapshot?.credential == .connected
    return Section {
      accessRow("Contents", status: repository.contents, id: "github-contents-access")
      accessRow("GitHub Actions", status: repository.actions, id: "github-actions-access")
      LabeledContent("Access") {
        HStack {
          Button(connected ? "Manage Token…" : "Reconnect GitHub…") {
            store.send(.openRepositoryAccount)
          }
          .accessibilityIdentifier("github-repository-account")
          if connected {
            Button("Check Again") { store.send(.verifyAccess) }
              .accessibilityIdentifier("github-verify-access")
          }
        }
      }
    } header: {
      Text("Last Recorded Repository Access")
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
    case .available: "Read access verified"
    case .unavailable: "Access unverified"
    case .unchecked: "Awaiting verification"
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
