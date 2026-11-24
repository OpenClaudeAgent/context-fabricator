import ComposableArchitecture
import DesktopFeature
import Foundation
import LocalAPI
import SwiftUI

struct GitHubPickerInput {
  var filter = ""
  var repositoryURL = ""
  var selected: Int64?
  var enterURL = false
}

struct GitHubPickerSheet: View {
  let store: StoreOf<GitHubConnector>
  @Binding var input: GitHubPickerInput
  @FocusState private var focus: Field?

  private enum Field { case search, results, url }

  private var matches: [Components.Schemas.GitHubRepository] {
    let configured = Set(store.snapshot?.repositories.map(\.repository.id) ?? [])
    return store.candidates.filter {
      !configured.contains($0.id)
        && (input.filter.isEmpty || $0.fullName.localizedCaseInsensitiveContains(input.filter))
    }
  }

  private var canContinue: Bool {
    if input.enterURL {
      return !input.repositoryURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
    return matches.contains { $0.id == input.selected }
  }

  private var loadingCandidates: Bool {
    !input.enterURL && store.busy && store.candidates.isEmpty
  }

  var body: some View {
    GitHubSheet(
      title: "Add Repository", message: store.feedback, busy: store.busy && !loadingCandidates
    ) {
      VStack(alignment: .leading, spacing: 12) {
        if input.enterURL { urlEntry } else { browse }
        Button(input.enterURL ? "Browse Repositories" : "Use a Repository URL…") {
          input.enterURL.toggle()
          focus = input.enterURL ? .url : .search
        }
        .accessibilityIdentifier("github-picker-url-mode")
      }
      .padding(20)
    } buttons: {
      Spacer()
      Button("Cancel", role: .cancel) { store.send(.dismissPanel) }
        .keyboardShortcut(.cancelAction)
        .disabled(store.busy)
      Button("Continue") { continueSelection() }
        .keyboardShortcut(.defaultAction)
        .disabled(store.busy || !canContinue)
        .accessibilityIdentifier("github-continue")
    }
    .defaultFocus($focus, input.enterURL ? .url : .search)
    .onChange(of: store.busy, initial: true) { _, busy in
      if !busy, focus == nil { focus = input.enterURL ? .url : .search }
    }
  }

  private var browse: some View {
    Group {
      TextField("Search loaded repositories", text: $input.filter)
        .textFieldStyle(.roundedBorder)
        .focused($focus, equals: .search)
        .accessibilityIdentifier("github-picker-search")
        .onKeyPress(.downArrow) { moveSelection(by: 1) }
        .onKeyPress(.upArrow) { moveSelection(by: -1) }
        .onSubmit { continueSelection() }
      candidateList
      HStack {
        if store.candidates.isEmpty {
          Button("Reload Repositories") { store.send(.discover) }
            .disabled(store.busy)
        }
        Spacer()
        if store.nextPage != nil {
          Button("Load More") { store.send(.loadMore) }
            .accessibilityIdentifier("github-load-more")
        }
      }
    }
  }

  private var urlEntry: some View {
    VStack(alignment: .leading, spacing: 8) {
      TextField("Repository URL or owner/name", text: $input.repositoryURL)
        .textFieldStyle(.roundedBorder)
        .focused($focus, equals: .url)
        .onSubmit { continueSelection() }
        .accessibilityIdentifier("github-repository")
      Text("Use a GitHub repository URL or enter owner/name.")
        .font(.callout)
        .foregroundStyle(.secondary)
    }
  }

  private var candidateList: some View {
    ScrollViewReader { scroller in
      List(selection: $input.selected) {
        ForEach(matches, id: \.id) { candidate in
          HStack(spacing: 9) {
            Image(systemName: candidate._private ? "lock" : "book.closed")
              .foregroundStyle(.secondary)
              .accessibilityHidden(true)
            Text(candidate.fullName)
              .lineLimit(2)
            Spacer(minLength: 0)
          }
          .padding(.vertical, 4)
          .contentShape(Rectangle())
          .accessibilityElement(children: .combine)
          .accessibilityIdentifier("github-candidate-\(candidate.id)")
          .tag(candidate.id)
        }
      }
      .focused($focus, equals: .results)
      .onKeyPress(.downArrow) { moveSelection(by: 1) }
      .onKeyPress(.upArrow) { moveSelection(by: -1) }
      .onKeyPress(.return) {
        guard canContinue, !store.busy else { return .ignored }
        continueSelection()
        return .handled
      }
      .onChange(of: input.selected) { _, selected in
        if let selected { scroller.scrollTo(selected) }
      }
      .listStyle(.bordered)
    }
    .frame(minHeight: 150, idealHeight: 206, maxHeight: 280)
    .overlay {
      if loadingCandidates {
        LoadingStateView("Loading repositories…")
      } else if matches.isEmpty, !store.busy {
        if input.filter.isEmpty {
          Text("No repositories in the loaded list.")
            .font(.callout)
            .foregroundStyle(.secondary)
            .padding()
        } else {
          ContentUnavailableView.search(text: input.filter)
        }
      }
    }
  }

  private func moveSelection(by offset: Int) -> KeyPress.Result {
    let candidates = matches
    guard !store.busy, !candidates.isEmpty else { return .ignored }
    let index = candidates.firstIndex { $0.id == input.selected }
    let next =
      index.map { min(max($0 + offset, 0), candidates.count - 1) }
      ?? (offset > 0 ? 0 : candidates.count - 1)
    input.selected = candidates[next].id
    focus = .results
    return .handled
  }

  private func continueSelection() {
    guard canContinue, !store.busy else { return }
    if input.enterURL {
      store.send(
        .chooseRepository(input.repositoryURL.trimmingCharacters(in: .whitespacesAndNewlines)))
    } else if let candidate = matches.first(where: { $0.id == input.selected }) {
      store.send(.select(candidate))
    }
  }
}
