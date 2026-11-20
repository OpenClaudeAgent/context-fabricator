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
            Menu("Create Token", systemImage: "arrow.up.right") {
              if let url = URL(string: snapshot.authorizationUrl) {
                Link("Repository contents", destination: url)
              }
              if let url = URL(string: snapshot.actionsAuthorizationUrl) {
                Link("Repository contents and Actions", destination: url)
              }
            }
          }
          SecureField("Personal access token", text: $token, prompt: Text("github_pat_…"))
            .accessibilityIdentifier("github-token")
        } footer: {
          Text(
            "Choose the repositories on GitHub, then paste your token here. It’s stored in Keychain."
          )
        }
      }
      .formStyle(.grouped)
      .contentMargins(.bottom, 14, for: .scrollContent)
      .frame(minHeight: 180, idealHeight: snapshot.login == nil ? 210 : 260, maxHeight: 340)
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
