import ComposableArchitecture
import DesktopFeature
import LocalAPI
import SwiftUI

struct GitHubActionsSheet: View {
  @Environment(\.locale) private var locale
  let store: StoreOf<GitHubConnector>

  private var repository: Components.Schemas.GitHubConfiguredRepository? {
    store.snapshot?.repositories.first { $0.repository.id == store.actionsRepositoryId }
  }

  var body: some View {
    GitHubSheet(
      title: "GitHub Actions",
      subtitle: repository.map { "\($0.repository.fullName) · \($0.branch)" },
      message: store.feedback, busy: store.busy && store.actionsState != nil,
      width: .wide
    ) {
      Group {
        if store.actionsState == nil && store.busy {
          LoadingStateView("Loading Actions…")
        } else {
          activity
        }
      }
      .frame(minHeight: 480, idealHeight: 550, maxHeight: 550)
    } buttons: {
      Button("Back") { store.send(.dismissPanel) }
        .disabled(store.busy)
        .accessibilityIdentifier("github-actions-back")
      Spacer()
      primaryAction
    }
  }

  @ViewBuilder private var primaryAction: some View {
    switch store.actionsState?.availability {
    case .accessRequired:
      Button("Check Access", systemImage: "checkmark.shield") { store.send(.verifyAccess) }
        .buttonStyle(.borderedProminent)
        .disabled(store.busy || store.snapshot?.credential != .connected)
        .accessibilityIdentifier("github-actions-check-access")
    case .reconnectRequired:
      Button("Reconnect GitHub…") { store.send(.openRepositoryAccount) }
        .buttonStyle(.borderedProminent)
        .disabled(store.busy)
    default:
      Button("Refresh Now", systemImage: "arrow.clockwise") { store.send(.refreshActions) }
        .disabled(store.busy || store.actionsState?.availability != .ready)
        .accessibilityIdentifier("github-refresh-actions")
    }
  }

  @ViewBuilder private var activity: some View {
    if let state = store.actionsState {
      if let snapshot = state.snapshot {
        GitHubActionsActivity(snapshot: snapshot).id(snapshot.repositoryId)
      } else {
        ContentUnavailableView(
          state.availability == .ready ? "No Runs Collected" : "Actions Access Required",
          systemImage: "play.rectangle",
          description: Text(availability(state))
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
      }
    } else {
      ContentUnavailableView("Actions Unavailable", systemImage: "exclamationmark.circle")
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
  }

  private func availability(_ state: Components.Schemas.GitHubActionsState) -> String {
    switch state.availability {
    case .ready: locale.localized("Workflow runs will appear here after collection.")
    case .disabled: locale.localized("Actions collection is off")
    case .reconnectRequired: locale.localized("Reconnect GitHub to collect Actions")
    case .accessRequired:
      repository?.verified == false
        ? "Repository access needs to be checked with the current token."
        : locale.localized("GitHub requires Contents and Actions read access for this repository.")
    }
  }
}
