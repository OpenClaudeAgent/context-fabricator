import ComposableArchitecture
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func branchPickerPagesOnlyOnDemandAndKeepsTheChoiceInTheDraft() async {
  let store = TestStore(initialState: branchSettings()) {
    GitHubConnector { command in
      switch command {
      case .branches(id: 41, revision: 2, page: 1):
        return .branches(.init(branches: ["main", "feature/search"], nextPage: 2))
      case .branches(id: 41, revision: 2, page: 2):
        return .branches(.init(branches: ["feature/search", "release/v1"]))
      default:
        Issue.record("The branch picker should only read the requested branch pages.")
        throw BackendConnectionError.helperExited
      }
    }
  }
  let target = GitHubBranchPicker.Target(id: 41, revision: 2)
  await store.send(.openBranchPicker)
  await store.receive(\.branchPicker.open) {
    $0.branchPicker.target = target
    $0.branchPicker.phase = .loading(1)
  }
  await store.receive(\.branchPicker.loaded) {
    $0.branchPicker.phase = .ready
    $0.branchPicker.branches = ["main", "feature/search"]
    $0.branchPicker.nextPage = 2
  }
  #expect(store.state.branchPicker.matchingBranches(" SEARCH ") == ["feature/search"])
  #expect(store.state.branchPicker.matchingBranches("release").isEmpty)
  await store.send(.openBranchPicker)
  await store.receive(\.branchPicker.open)
  await store.send(.branchPicker(.loadMore)) { $0.branchPicker.phase = .loading(2) }
  await store.receive(\.branchPicker.loaded) {
    $0.branchPicker.phase = .ready
    $0.branchPicker.branches = ["main", "feature/search", "release/v1"]
    $0.branchPicker.nextPage = nil
  }
  #expect(store.state.branchPicker.matchingBranches("release") == ["release/v1"])
  await store.send(.branchPicker(.select("release/v1"))) {
    $0.draft?.branch = "release/v1"
  }
  #expect(store.state.draft?.intervalMinutes == 90)
  #expect(store.state.snapshot?.repositories.first?.branch == "main")
  await store.send(.branchPicker(.close)) { $0.branchPicker = .init() }
}

@MainActor
@Test
func branchPickerRetryKeepsLoadedBranchesAndIgnoresOtherRepositories() async {
  let target = GitHubBranchPicker.Target(id: 41, revision: 2)
  let other = GitHubBranchPicker.Target(id: 42, revision: 2)
  let message = "GitHub is cooling down."
  let clock = TestClock()
  let store = TestStore(initialState: GitHubBranchPicker.State()) {
    GitHubBranchPicker { target, page in
      if page == 1 {
        if target == other { try await clock.sleep(for: .seconds(1)) }
        return .init(branches: ["main"], nextPage: 2)
      }
      throw GitHubRejection(code: "github_throttled", message: message)
    }
  }
  await store.send(.open(target)) {
    $0.target = target
    $0.phase = .loading(1)
  }
  await store.receive(\.loaded) {
    $0.phase = .ready
    $0.branches = ["main"]
    $0.nextPage = 2
  }
  await store.send(.loadMore) { $0.phase = .loading(2) }
  await store.receive(\.failed) { $0.phase = .failed(2, message) }
  #expect(store.state.branches == ["main"])
  await store.send(.retry) { $0.phase = .loading(2) }
  await store.receive(\.failed) { $0.phase = .failed(2, message) }
  await store.send(.open(other)) {
    $0 = .init()
    $0.target = other
    $0.phase = .loading(1)
  }
  await store.send(.loaded(target, page: 1, .init(branches: ["stale"])))
  await clock.advance(by: .seconds(1))
  await store.receive(\.loaded) {
    $0.phase = .ready
    $0.branches = ["main"]
    $0.nextPage = 2
  }
  await store.send(.close) { $0 = .init() }
  await store.send(.loaded(other, page: 1, .init(branches: ["late"])))
}

private func branchSettings() -> GitHubConnector.State {
  let repository = Components.Schemas.GitHubConfiguredRepository(
    repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: false),
    branch: "main", intervalMinutes: 60,
    features: .init(currentCode: true, history: false, actions: false, codeGraph: false),
    contents: .available, actions: .unchecked, verified: true)
  var state = GitHubConnector.State()
  state.panel = .editOptions
  state.draft = .init(repository)
  state.draft?.intervalInput = "90"
  state.draftBaseline = .init(revision: 2, configured: .init(repository))
  state.snapshot = .init(
    revision: 2, credential: .connected, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com",
    defaults: .init(intervalMinutes: 60, features: repository.features), repositories: [repository])
  return state
}
