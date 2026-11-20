import ComposableArchitecture
import DesktopFeature
import SwiftUI

struct GitHubSheetContent: View {
  let store: StoreOf<GitHubConnector>
  @State private var picker = GitHubPickerInput()

  var body: some View {
    switch store.panel {
    case .account, .repositoryAccount:
      if let snapshot = store.snapshot {
        GitHubAccountSheet(store: store, snapshot: snapshot)
      }
    case .picker:
      GitHubPickerSheet(store: store, input: $picker)
    case .addOptions, .editOptions:
      if let draft = store.draft {
        GitHubOptionsSheet(store: store, draft: draft)
      }
    case .traffic:
      if let traffic = store.snapshot?.traffic {
        GitHubTrafficSheet(store: store, traffic: traffic)
      }
    case nil:
      EmptyView()
    }
  }
}

struct GitHubSheet<Content: View, Buttons: View>: View {
  let title: LocalizedStringKey
  let message: String?
  let busy: Bool
  @ViewBuilder let content: Content
  @ViewBuilder let buttons: Buttons

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: 12) {
        GitHubIcon(size: 36)
        Text(title).font(.title2.bold()).accessibilityAddTraits(.isHeader)
        Spacer()
      }
      .padding(.horizontal, 22)
      .padding(.vertical, 18)
      Divider()
      content.disabled(busy)
      Divider()
      VStack(alignment: .leading, spacing: 10) {
        if let message { GitHubFeedback(message: message) }
        HStack {
          if busy { ProgressView().controlSize(.small).accessibilityLabel("Working") }
          buttons
        }
      }
      .padding(.horizontal, 22)
      .padding(.vertical, 14)
    }
    .frame(minWidth: 450, idealWidth: 540, maxWidth: 640)
  }
}
