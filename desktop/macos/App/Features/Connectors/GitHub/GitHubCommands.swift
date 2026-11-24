import ComposableArchitecture
import DesktopFeature
import SwiftUI

struct GitHubWorkspaceActions {
  let store: StoreOf<GitHubConnector>
  let searching: Binding<Bool>
}

private struct GitHubWorkspaceKey: FocusedValueKey {
  typealias Value = GitHubWorkspaceActions
}

extension FocusedValues {
  var githubWorkspace: GitHubWorkspaceActions? {
    get { self[GitHubWorkspaceKey.self] }
    set { self[GitHubWorkspaceKey.self] = newValue }
  }
}

struct GitHubCommands: Commands {
  let locale: Locale
  @FocusedValue(\.githubWorkspace) private var workspace

  var body: some Commands {
    CommandMenu(locale.localized("GitHub")) {
      Button(locale.localized("Back to Connectors")) { workspace?.store.send(.close) }
        .disabled(workspace == nil || workspace?.store.panel != nil)
      Button(locale.localized("Reload Settings")) { workspace?.store.send(.reload) }
        .keyboardShortcut("r")
        .disabled(!canInteract)
      Button(locale.localized("Find Repositories")) { workspace?.searching.wrappedValue = true }
        .keyboardShortcut("f")
        .disabled(!canInteract || workspace?.store.snapshot?.repositories.isEmpty != false)
      Button(locale.localized("Add Repository…")) { workspace?.store.send(.add) }
        .disabled(!canInteract || workspace?.store.snapshot?.credential != .connected)
    }
  }

  private var canInteract: Bool {
    workspace?.store.busy == false && workspace?.store.panel == nil
  }
}
