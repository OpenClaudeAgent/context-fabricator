import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@Test
func generatedClientConfiguresTheLiveGitHubFixture() async throws {
  let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
  try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  let fixture = Process()
  let backend = Process()
  defer {
    for process in [backend, fixture] where process.isRunning {
      process.terminate()
      process.waitUntilExit()
    }
    try? FileManager.default.removeItem(at: directory)
  }
  let script = URL(filePath: #filePath).deletingLastPathComponent()
    .appending(path: "../../../../../quality/m07/github_fixture.py").standardizedFileURL
  try start(fixture, executable: "/usr/bin/python3", arguments: [script.path, directory.path])
  let port = try await readFile(directory.appending(path: "port"))
  let executable = try #require(
    ProcessInfo.processInfo.environment["CTXFAB_TEST_BACKEND_EXECUTABLE"])
  let data = directory.appending(path: "data")
  try start(
    backend, executable: executable,
    arguments: [
      data.path, "--github-fixture", String(decoding: port, as: UTF8.self),
    ])
  let endpointData = try await readFile(data.appending(path: "endpoint.json"))
  struct Endpoint: Decodable {
    let url: URL
    let token: String
  }
  let endpoint = try JSONDecoder().decode(Endpoint.self, from: endpointData)
  let session = SessionClient(url: endpoint.url, token: endpoint.token)
  try await verifyCommands(session)
}

private func verifyCommands(_ session: SessionClient) async throws {
  let initial = try await GitHubCommand.load.perform(using: session)
  guard case .snapshot(let start) = initial else {
    Issue.record("Expected initial GitHub state.")
    return
  }
  #expect(start.credential == .unconfigured)
  let token = GitHubToken("github_pat_fixture_primary")
  #expect(!String(reflecting: token).contains("github_pat_"))
  let authorization = try await GitHubCommand.authorize(token, revision: start.revision)
    .perform(using: session)
  guard case .credential(let connected) = authorization else {
    Issue.record("Expected connected GitHub state.")
    return
  }
  #expect(connected.credential == .connected)
  let discovery = try await GitHubCommand.discover(page: 1).perform(using: session)
  guard case .page(let page, replace: true) = discovery else {
    Issue.record("Expected the first GitHub repository page.")
    return
  }
  let repository = try #require(page.repositories.first)
  let selection = Components.Schemas.GitHubSelection(
    revision: connected.revision, repository: repository.fullName, expectedId: repository.id,
    intervalMinutes: connected.defaults.intervalMinutes, features: connected.defaults.features)
  guard case .saved(let saved) = try await GitHubCommand.save(selection).perform(using: session)
  else {
    Issue.record("Expected saved repository.")
    return
  }
  #expect(saved.repositories.first?.contents == .available)
  #expect(try await session.status().connectedSources == 1)
  let queued = try await verifyCodeQueue(saved, repositoryID: repository.id, using: session)
  let verified = try await verifySavedAccess(queued, using: session)
  let removal = GitHubCommand.remove(id: repository.id, revision: verified.revision)
  guard case .removed(let removed) = try await removal.perform(using: session) else {
    Issue.record("Expected removed repository.")
    return
  }
  #expect(removed.repositories.isEmpty)
  let disconnect = GitHubCommand.disconnect(revision: removed.revision)
  guard case .credential(let disconnected) = try await disconnect.perform(using: session) else {
    Issue.record("Expected disconnected credential.")
    return
  }
  #expect(disconnected.credential == .unconfigured)
  try await verifyRejections(session, selection: selection)
  try await verifyRateLimit(disconnected, using: session)
}

private func verifyCodeQueue(
  _ saved: Components.Schemas.GitHubState, repositoryID: Int64, using session: SessionClient
) async throws -> Components.Schemas.GitHubState {
  guard
    case .snapshot(let queued) = try await GitHubCommand.refreshCode(repositoryID)
      .perform(using: session)
  else {
    throw URLError(.badServerResponse)
  }
  #expect(queued.repositories.first?.codeCollection?.phase == .queued)
  #expect(queued.repositories.first?.codeCollection?.nextAtMs != nil)
  #expect(queued.traffic?.attempts == saved.traffic?.attempts)
  return queued
}

private func verifyRateLimit(_ state: Components.Schemas.GitHubState, using session: SessionClient)
  async throws
{
  let command = GitHubCommand.setRate(requestsPerMinute: 30, revision: state.revision)
  if case .snapshot(let updated) = try await command.perform(using: session) {
    #expect(updated.traffic?.effectiveRequestsPerMinute == 30)
    #expect(updated.traffic?.attempts == state.traffic?.attempts)
  } else {
    Issue.record("Expected the updated outbound rate.")
  }
}

private func verifySavedAccess(
  _ saved: Components.Schemas.GitHubState, using session: SessionClient
)
  async throws -> Components.Schemas.GitHubState
{
  let configured = try #require(saved.repositories.first)
  let verification = GitHubCommand.verify(
    GitHubConnector.Draft(configured).selection(revision: saved.revision))
  let result = try await verification.perform(using: session)
  let snapshot: Components.Schemas.GitHubState? =
    if case .verified(let snapshot) = result { snapshot } else { nil }
  let verified = try #require(snapshot)
  #expect(verified.repositories == saved.repositories)
  return verified
}

private func verifyRejections(
  _ session: SessionClient, selection: Components.Schemas.GitHubSelection
) async throws {
  for (command, code) in [
    (GitHubCommand.authorize(.init("invalid"), revision: 0), "github_invalid_token"),
    (.discover(page: 1), "github_reconnect"),
    (.save(selection), "github_revision_conflict"),
    (.disconnect(revision: 0), "github_revision_conflict"),
    (.remove(id: 0, revision: 0), "github_invalid_selection"),
  ] {
    do {
      _ = try await command.perform(using: session)
      Issue.record("Expected typed GitHub rejection.")
    } catch let failure as GitHubRejection {
      #expect(failure.code == code)
      #expect(failure.errorDescription == failure.message)
    }
  }
}

private func start(_ process: Process, executable: String, arguments: [String]) throws {
  process.executableURL = URL(filePath: executable)
  process.arguments = arguments
  process.standardOutput = FileHandle.nullDevice
  process.standardError = FileHandle.nullDevice
  try process.run()
}

private func readFile(_ url: URL) async throws -> Data {
  let deadline = ContinuousClock.now + .seconds(5)
  while !FileManager.default.fileExists(atPath: url.path), ContinuousClock.now < deadline {
    try await Task.sleep(for: .milliseconds(30))
  }
  return try Data(contentsOf: url)
}

@MainActor
@Test
func repositoryPickerPreservesThePerRepositoryDraft() async {
  let features = Components.Schemas.GitHubFeatures(
    currentCode: true, history: false, actions: false, codeGraph: false)
  let defaults = Components.Schemas.GitHubDefaults(intervalMinutes: 60, features: features)
  let snapshot = Components.Schemas.GitHubState(
    revision: 1, credential: .connected, login: "fixture",
    authorizationUrl: "https://github.com", actionsAuthorizationUrl: "https://github.com",
    defaults: defaults, repositories: [])
  let a = Components.Schemas.GitHubRepository(
    id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: true)
  let b = Components.Schemas.GitHubRepository(
    id: 42, fullName: "fixture/beta", defaultBranch: "trunk", _private: true)
  let store = picker(snapshot: snapshot, first: a, second: b)
  await store.send(.manage) {
    $0.managing = true
    $0.busy = true
  }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = snapshot
  }
  await store.send(.discover) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.candidates = [a]
    $0.nextPage = 2
  }
  await store.send(.loadMore) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.candidates = [a, b]
    $0.nextPage = nil
  }
  await store.send(.add) {
    $0.panel = .picker
    $0.draft = .init(defaults: defaults)
  }
  await configureSecondRepository(b, in: store)
  await store.send(.close) { $0.managing = false }
}

@MainActor
private func configureSecondRepository(
  _ b: Components.Schemas.GitHubRepository, in store: TestStoreOf<GitHubConnector>
) async {
  await store.send(.select(b)) {
    $0.draft?.repository = b.fullName
    $0.draft?.expectedId = b.id
    $0.draft?.branch = b.defaultBranch
    $0.panel = .addOptions
  }
  var edited = store.state.draft
  edited?.features.actions = true
  if let edited { await store.send(.change(edited)) { $0.draft = edited } }
  await store.send(.save) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.draft = nil
    $0.panel = nil
  }
}

@MainActor
private func picker(
  snapshot: Components.Schemas.GitHubState,
  first: Components.Schemas.GitHubRepository,
  second: Components.Schemas.GitHubRepository
) -> TestStoreOf<GitHubConnector> {
  TestStore(initialState: GitHubConnector.State()) {
    GitHubConnector { command in
      switch command {
      case .discover(let page):
        return page == 1
          ? .page(.init(repositories: [first], nextPage: 2), replace: true)
          : .page(.init(repositories: [first, second]), replace: false)
      case .save(let selection):
        #expect(selection.expectedId == second.id)
        #expect(selection.branch == second.defaultBranch)
        #expect(selection.features.actions)
        return .saved(snapshot)
      default:
        return .snapshot(snapshot)
      }
    }
  }
}

@MainActor
@Test
func collectionPollDoesNotRebaseAnOpenRepositoryDraft() async {
  let features = Components.Schemas.GitHubFeatures(
    currentCode: true, history: false, actions: false, codeGraph: false)
  let configured = Components.Schemas.GitHubConfiguredRepository(
    repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: true),
    branch: "main", intervalMinutes: 60, features: features,
    contents: .available, actions: .unchecked, verified: true
  )
  let snapshot = Components.Schemas.GitHubState(
    revision: 2, credential: .connected, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com",
    defaults: .init(intervalMinutes: 60, features: features),
    repositories: [configured]
  )
  var initial = GitHubConnector.State()
  initial.managing = true
  initial.snapshot = snapshot
  initial.draft = .init(configured)
  initial.panel = .editOptions
  let store = TestStore(initialState: initial) {
    GitHubConnector { _ in throw BackendConnectionError.helperExited }
  }
  var concurrent = snapshot
  concurrent.revision = 3
  concurrent.repositories[0].branch = "release"
  await store.send(.collectionPolled(concurrent))
  let conflict = "GitHub settings changed. Refresh and try again."
  await store.send(.result(.snapshot(concurrent), feedback: conflict)) {
    $0.feedback = conflict
  }
  var refreshed = snapshot
  refreshed.repositories[0].codeCollection = .init(phase: .running, canRefresh: false)
  await store.send(.collectionPolled(refreshed)) {
    $0.snapshot = refreshed
  }
}

@MainActor
@Test
func remoteExpiryRefreshesTheVisibleCredentialState() async {
  let defaults = Components.Schemas.GitHubDefaults(
    intervalMinutes: 60,
    features: .init(currentCode: true, history: false, actions: false, codeGraph: false))
  let active = Components.Schemas.GitHubState(
    revision: 1, credential: .connected, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com", defaults: defaults, repositories: [])
  var expired = active
  expired.revision = 2
  expired.credential = .reconnectRequired
  let final = expired
  let message = "Connect GitHub with a valid token."
  var initial = GitHubConnector.State()
  initial.snapshot = active
  initial.nextPage = 2
  let store = TestStore(initialState: initial) {
    GitHubConnector { command in
      switch command {
      case .load: return .snapshot(final)
      case .discover: throw GitHubRejection(code: "github_reconnect", message: message)
      case .authorize: throw GitHubRejection(code: "github_invalid_token", message: message)
      default: throw BackendConnectionError.helperExited
      }
    }
  }
  await store.send(.discover) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.feedback = message
    $0.snapshot = final
    $0.nextPage = nil
  }
  await store.send(.authorize(.init("invalid"))) {
    $0.busy = true
    $0.feedback = nil
  }
  await store.receive(\.failed) {
    $0.busy = false
    $0.feedback = message
  }
}

@MainActor
@Test
func credentialAndRepositoryCommandsAdoptTheBackendState() async {
  let features = Components.Schemas.GitHubFeatures(
    currentCode: true, history: false, actions: false, codeGraph: false)
  let repo = Components.Schemas.GitHubConfiguredRepository(
    repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: true),
    branch: "release", intervalMinutes: 90, features: features,
    contents: .available, actions: .unchecked, verified: true)
  let initial = Components.Schemas.GitHubState(
    revision: 0, credential: .unconfigured, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com",
    defaults: .init(
      intervalMinutes: 60, features: features),
    repositories: [])
  var authorized = initial
  authorized.revision = 1
  authorized.credential = .connected
  authorized.repositories = [repo]
  let active = authorized
  authorized.revision = 2
  authorized.repositories = []
  let removed = authorized
  let store = manager(initial: initial, active: active, removed: removed)
  await store.send(.reload) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = initial
  }
  await store.send(.authorize(.init("test"))) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = active
  }
  await store.send(.edit(repo)) {
    $0.draft = .init(repo)
    $0.panel = .editOptions
  }
  await store.send(.dismissPanel) {
    $0.draft = nil
    $0.panel = nil
  }
  await store.send(.remove(41)) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = removed
  }
  await store.send(.disconnect) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = initial
  }
}

@MainActor
private func manager(
  initial: Components.Schemas.GitHubState,
  active: Components.Schemas.GitHubState,
  removed: Components.Schemas.GitHubState
) -> TestStoreOf<GitHubConnector> {
  TestStore(initialState: GitHubConnector.State()) {
    GitHubConnector { command in
      switch command {
      case .authorize(_, revision: 0): return .credential(active)
      case .remove(id: 41, revision: 1): return .removed(removed)
      case .disconnect(revision: 2): return .credential(initial)
      default: return .snapshot(initial)
      }
    }
  }
}

@MainActor
@Test
func directRepositoryEntryStaysInTheSameSheetUntilDismissed() async {
  let defaults = Components.Schemas.GitHubDefaults(
    intervalMinutes: 60,
    features: .init(currentCode: true, history: false, actions: false, codeGraph: false))
  var state = GitHubConnector.State()
  state.snapshot = .init(
    revision: 1, credential: .connected, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com", defaults: defaults, repositories: [])
  let store = TestStore(initialState: state) {
    GitHubConnector { command in
      if case .discover = command { return .page(.init(repositories: []), replace: true) }
      throw GitHubRejection(code: "github_invalid_token", message: "Token rejected.")
    }
  }
  await store.send(.add) {
    $0.panel = .picker
    $0.draft = .init(defaults: defaults)
    $0.busy = true
  }
  await store.receive(\.result) { $0.busy = false }
  let entered = "https://github.com/fixture/example"
  await store.send(.chooseRepository(entered)) {
    $0.draft?.repository = entered
    $0.panel = .addOptions
  }
  await store.send(.backToPicker) { $0.panel = .picker }
  await store.send(.dismissPanel) {
    $0.draft = nil
    $0.panel = nil
  }
  await store.send(.openAccount) { $0.panel = .account }
  await store.send(.authorize(.init("invalid"))) { $0.busy = true }
  await store.receive(\.failed) {
    $0.busy = false
    $0.feedback = "Token rejected."
  }
}
