import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct GitHubAccountSheet: View {
  let store: StoreOf<GitHubConnector>
  let snapshot: Components.Schemas.GitHubState
  @State private var token = ""
  @State private var disconnect = false

  private var tokenSetup: Components.Schemas.GitHubTokenSetup? {
    guard store.panel == .repositoryAccount,
      let id = store.draft?.expectedId ?? store.actionsRepositoryId
    else { return nil }
    return snapshot.repositories.first { $0.repository.id == id }?.tokenSetup
  }

  private var accountTitle: LocalizedStringKey {
    switch snapshot.credential {
    case .connected: "GitHub Account"
    case .reconnectRequired: "Reconnect GitHub"
    case .unconfigured: "Connect to GitHub"
    }
  }

  var body: some View {
    GitHubSheet(
      title: accountTitle,
      message: store.feedback, busy: store.busy
    ) {
      Form {
        if let login = snapshot.login {
          LabeledContent("Account", value: login)
        }
        Section {
          LabeledContent("GitHub") {
            Menu("Create Token") {
              if let url = URL(string: tokenSetup?.authorizationUrl ?? snapshot.authorizationUrl) {
                Link("Repository contents", destination: url)
              }
              if let url = URL(
                string: tokenSetup?.actionsAuthorizationUrl ?? snapshot.actionsAuthorizationUrl)
              {
                Link("Repository contents and Actions", destination: url)
              }
            }
          }
          SecureField("Personal access token", text: $token, prompt: Text("github_pat_…"))
            .accessibilityIdentifier("github-token")
        } footer: {
          VStack(alignment: .leading, spacing: 8) {
            Text(
              "On GitHub, set Resource owner to the account or organization that owns the repository. Select the repository under Repository access."
            )
            Text("If GitHub marks the token as pending, an organization admin must approve it.")
            if snapshot.credential == .connected {
              Text("Replacing the token can affect access to repositories under other owners.")
            }
          }
        }
      }
      .formStyle(.grouped)
      .contentMargins(.bottom, 14, for: .scrollContent)
      .frame(minHeight: 230, idealHeight: snapshot.login == nil ? 260 : 340, maxHeight: 400)
    } buttons: {
      if snapshot.credential != .unconfigured {
        Button("Disconnect…", role: .destructive) { disconnect = true }
          .disabled(store.busy)
          .accessibilityIdentifier("github-disconnect")
      }
      Spacer()
      Button("Cancel", role: .cancel) { store.send(.dismissPanel) }
        .keyboardShortcut(.cancelAction)
        .disabled(store.busy)
      Button(snapshot.credential == .connected ? "Replace Token" : "Connect") {
        let submitted = GitHubToken(token)
        token = ""
        store.send(.authorize(submitted))
      }
      .keyboardShortcut(.defaultAction)
      .disabled(token.isEmpty || store.busy)
      .accessibilityIdentifier("github-authorize")
    }
    .alert("Disconnect GitHub?", isPresented: $disconnect) {
      Button("Cancel", role: .cancel) {}
      Button("Disconnect", role: .destructive) { store.send(.disconnect) }
    } message: {
      Text("Your saved repository settings will be available when you reconnect.")
    }
  }
}
