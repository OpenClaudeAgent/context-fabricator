import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func actionsPanelLoadsRefreshesAndKeepsRepositoryContext() async {
  let loaded = Components.Schemas.GitHubActionsState(availability: .ready)
  var initial = GitHubConnector.State()
  initial.panel = .editOptions
  let store = TestStore(initialState: initial) {
    GitHubConnector { command in
      guard case .actions(let id, _) = command else { throw BackendConnectionError.helperExited }
      return .actions(id, loaded)
    }
  }
  await store.send(.openActions(41)) {
    $0.actionsRepositoryId = 41
    $0.panel = .actions
    $0.busy = true
  }
  await store.receive(\.result) {
    $0.busy = false
    $0.actionsState = loaded
  }
  await store.send(.refreshActions) { $0.busy = true }
  await store.receive(\.result) { $0.busy = false }
  await store.send(.dismissPanel) { $0.panel = .editOptions }
}

@MainActor
@Test
func actionsRefreshRespectsUnavailableAccess() async {
  var initial = GitHubConnector.State()
  initial.panel = .actions
  initial.actionsRepositoryId = 41
  initial.actionsState = .init(availability: .accessRequired)
  let store = TestStore(initialState: initial) {
    GitHubConnector { _ in throw BackendConnectionError.helperExited }
  }
  await store.send(.refreshActions)
}

@MainActor
@Test(arguments: [false, true])
func actionsRefreshFromRepositorySettingsKeepsUnsavedEdits(rejected: Bool) async {
  let status = Components.Schemas.GitHubActionsState(
    availability: .ready, schedule: .init(nextAtMs: 123_000, lastCheckedAtMs: 63_000))
  let savedStatus = Components.Schemas.GitHubActionsState(
    availability: .ready, schedule: .init(nextAtMs: 111_000, lastCheckedAtMs: 51_000))
  let store = TestStore(initialState: actionsRepositorySettingsState()) {
    GitHubConnector { command in
      switch command {
      case .repositorySettings(41):
        return .repositorySettings(41, paused: true, actions: savedStatus)
      case .actions(41, true):
        if rejected {
          throw GitHubRejection(code: "github_unavailable", message: "GitHub is unavailable")
        }
        return .actions(41, status)
      case .actions(41, false) where rejected:
        return .actions(41, status)
      case .dataState(41) where rejected:
        return .dataState(41, false)
      case .load where rejected:
        throw BackendConnectionError.helperExited
      default:
        Issue.record("Unexpected command for a repository Actions refresh")
        throw BackendConnectionError.helperExited
      }
    }
  }
  await store.send(.loadDataState(41)) {
    $0.dataStateId = 41
    $0.busy = true
  }
  await store.receive(\.result) {
    $0.busy = false
    $0.dataPaused = true
    $0.actionsRepositoryId = 41
    $0.actionsState = savedStatus
  }
  await store.send(.refreshActions) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.dataPaused = false
    $0.actionsState = status
    $0.feedback = rejected ? "GitHub is unavailable" : "Workflow runs refreshed."
  }
}

private func actionsRepositorySettingsState() -> GitHubConnector.State {
  let repository = Components.Schemas.GitHubConfiguredRepository(
    repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: false),
    branch: "main", intervalMinutes: 60,
    features: .init(currentCode: true, history: false, actions: true, codeGraph: false),
    contents: .available, actions: .available, verified: true)
  var state = GitHubConnector.State()
  state.panel = .editOptions
  state.draft = .init(repository)
  state.draft?.branch = "unsaved-branch"
  state.draftBaseline = .init(revision: 1, configured: .init(repository))
  state.snapshot = .init(
    revision: 1, credential: .connected, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com",
    defaults: .init(intervalMinutes: 60, features: repository.features), repositories: [repository])
  return state
}

@MainActor
@Test
func actionsPanelUpdatesStatusAfterRejectedRefresh() async {
  let current = Components.Schemas.GitHubActionsState(
    availability: .ready,
    schedule: .init(nextAtMs: 0, lastCheckedAtMs: 1_000, lastError: .unavailable))
  var initial = GitHubConnector.State()
  initial.panel = .actions
  initial.actionsRepositoryId = 41
  initial.actionsState = .init(availability: .ready)
  let store = TestStore(initialState: initial) {
    GitHubConnector { command in
      switch command {
      case .actions(let id, true):
        #expect(id == 41)
        throw GitHubRejection(code: "github_unavailable", message: "GitHub is unavailable")
      case .actions(let id, false):
        #expect(id == 41)
        return .actions(id, current)
      case .load:
        throw BackendConnectionError.helperExited
      default:
        Issue.record("Unexpected command during Actions recovery")
        throw BackendConnectionError.helperExited
      }
    }
  }
  await store.send(.refreshActions) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.actionsState = current
    $0.feedback = "GitHub is unavailable"
  }
}

@MainActor
@Test(arguments: [true, false])
func actionsPanelRechecksSavedAccess(allowed: Bool) async {
  let features = Components.Schemas.GitHubFeatures(
    currentCode: true, history: false, actions: true, codeGraph: false)
  let repository = Components.Schemas.GitHubConfiguredRepository(
    repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: false),
    branch: "stable", intervalMinutes: 90, features: features,
    contents: .unchecked, actions: .unchecked, verified: false)
  let snapshot = Components.Schemas.GitHubState(
    revision: 3, credential: .connected, login: "fixture",
    authorizationUrl: "https://github.com", actionsAuthorizationUrl: "https://github.com",
    defaults: .init(intervalMinutes: 60, features: features), repositories: [repository])
  var verified = snapshot
  verified.revision = 4
  verified.repositories[0].contents = .available
  verified.repositories[0].actions = allowed ? .available : .unavailable
  verified.repositories[0].verified = true
  let checkedSnapshot = verified
  let checkedActions = Components.Schemas.GitHubActionsState(
    availability: allowed ? .ready : .accessRequired)
  var initial = GitHubConnector.State()
  initial.snapshot = snapshot
  initial.panel = .actions
  initial.actionsRepositoryId = 41
  initial.actionsState = .init(availability: .accessRequired)
  let store = TestStore(initialState: initial) {
    GitHubConnector { command in
      switch command {
      case .verify(let id, let revision):
        #expect(id == 41)
        #expect(revision == 3)
        return .verified(checkedSnapshot)
      case .actions(let id, let refresh):
        #expect(id == 41)
        #expect(!refresh)
        return .actions(id, checkedActions)
      default:
        Issue.record("Unexpected command during the Actions access check")
        throw BackendConnectionError.helperExited
      }
    }
  }
  await store.send(.verifyAccess) { $0.busy = true }
  await store.receive(\.result) { $0.snapshot = checkedSnapshot }
  await store.receive(\.result) {
    $0.busy = false
    $0.actionsState = checkedActions
  }
}

@Test
func scheduledStorageLimitKeepsPreviousActionAttemptReadable() throws {
  let response = #"""
    {
      "availability": "ready",
      "schedule": { "nextAtMs": 2000, "lastCheckedAtMs": 1000, "lastError": "storageLimit" },
      "snapshot": {
        "repositoryId": 41, "repository": "fixture/alpha", "branch": "main",
        "definitionCommit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "workflowCoverage": "providerDefaultBranchList", "executedDefinition": "unknown",
        "runCoverage": "returnedBranchRuns", "attemptCoverage": "observedLatestAttemptsOnly",
        "workflows": [],
        "runs": [{
          "id": 7, "attempt": 2, "workflowId": 3, "event": "push",
          "headSha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "headBranch": "main",
          "headRepositoryId": 41, "commitProvenance": "verifiedRepositoryCommit",
          "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
          "status": "completed", "conclusion": "success"
        }]
      }
    }
    """#
  let decoder = JSONDecoder()
  decoder.dateDecodingStrategy = .iso8601
  let state = try decoder.decode(
    Components.Schemas.GitHubActionsState.self, from: Data(response.utf8))
  #expect(state.schedule?.lastError?.rawValue == "storageLimit")
  #expect(state.snapshot?.runs.first?.attempt == 2)
  #expect(state.snapshot?.runs.first?.conclusion == "success")
}
