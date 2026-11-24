import ComposableArchitecture
import DesktopFeature
import SwiftUI

struct GitHubBranchPickerView: View {
  let store: StoreOf<GitHubConnector>
  let selection: String

  @State private var presented = false
  @State private var query = ""
  @FocusState private var searching: Bool

  var body: some View {
    Button {
      query = ""
      store.send(.openBranchPicker)
      presented = true
    } label: {
      Label("Search branches", systemImage: "chevron.up.chevron.down")
        .labelStyle(.iconOnly)
    }
    .disabled(store.branchPickerTarget == nil || store.busy)
    .accessibilityValue(selection)
    .accessibilityIdentifier("github-branch-picker")
    .help("Search branches")
    .popover(isPresented: $presented, arrowEdge: .trailing) {
      VStack(spacing: 8) {
        searchField
        Divider()
        results
        pagination
      }
      .padding(12)
      .frame(width: 324, height: 350)
      .defaultFocus($searching, true)
      .onDisappear { store.send(.branchPicker(.close)) }
    }
    .onChange(of: store.branchPickerTarget) { _, _ in presented = false }
  }

  private var searchField: some View {
    HStack(spacing: 8) {
      Image(systemName: "magnifyingglass")
        .foregroundStyle(.secondary)
        .accessibilityHidden(true)
      TextField("Search branches", text: $query)
        .textFieldStyle(.plain)
        .multilineTextAlignment(.leading)
        .focused($searching)
        .accessibilityIdentifier("github-branch-search")
      if !query.isEmpty {
        Button("Clear search", systemImage: "xmark.circle.fill") { query = "" }
          .labelStyle(.iconOnly)
          .buttonStyle(.plain)
          .foregroundStyle(.secondary)
      }
    }
    .padding(.horizontal, 8)
    .padding(.vertical, 4)
  }

  private var results: some View {
    Group {
      switch store.branchPicker.phase {
      case .idle, .loading(1):
        ProgressView("Loading branches")
          .controlSize(.small)
          .accessibilityIdentifier("github-branch-loading")
      case .failed(1, let message):
        VStack(spacing: 8) {
          Text(message)
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)
          Button("Retry") { store.send(.branchPicker(.retry)) }
        }
        .padding(.horizontal, 8)
      default:
        let matches = store.branchPicker.matchingBranches(query)
        if matches.isEmpty {
          Text(store.branchPicker.branches.isEmpty ? "No branches found" : "No matching branches")
            .foregroundStyle(.secondary)
            .accessibilityIdentifier("github-branch-empty")
        } else {
          ScrollView {
            LazyVStack(spacing: 2) {
              ForEach(matches, id: \.self, content: branchRow)
            }
          }
        }
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }

  private var pagination: some View {
    Group {
      switch store.branchPicker.phase {
      case .loading(let page) where page > 1:
        ProgressView("Loading more branches")
          .controlSize(.small)
          .accessibilityIdentifier("github-branch-loading-more")
      case .failed(let page, let message) where page > 1:
        HStack {
          Text(message)
            .font(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(2)
            .help(message)
          Button("Retry") { store.send(.branchPicker(.retry)) }
        }
      case .ready where store.branchPicker.nextPage != nil:
        Button("Load More Branches") { store.send(.branchPicker(.loadMore)) }
          .accessibilityIdentifier("github-branch-load-more")
      default:
        Color.clear.accessibilityHidden(true)
      }
    }
    .frame(maxWidth: .infinity)
    .frame(height: 32)
  }

  private func branchRow(_ branch: String) -> some View {
    Button {
      store.send(.branchPicker(.select(branch)))
      presented = false
    } label: {
      HStack(spacing: 10) {
        Image(systemName: "checkmark")
          .frame(width: 14)
          .foregroundStyle(Color.accentColor)
          .opacity(branch == selection ? 1 : 0)
          .accessibilityHidden(true)
        Text(branch).lineLimit(1)
        Spacer(minLength: 0)
      }
      .padding(.horizontal, 8)
      .padding(.vertical, 5)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(branch == selection ? [.isSelected] : [])
    .accessibilityIdentifier("github-branch-option-\(branch)")
    .help(branch)
  }
}
