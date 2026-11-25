import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@Test
func generatedClientConfiguresTheLiveGitHubFixture() async throws {
  try await withGitHubBackendFixture { session, origin in
    try await verifyCommands(session, origin: origin)
  }
}

func withGitHubBackendFixture(
  answerWorker: Bool = false,
  publicBenchmark: Bool = false,
  run: (SessionClient, URL) async throws -> Void
) async throws {
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
    .appending(path: "../../../../../quality/github/github_fixture.py").standardizedFileURL
  let fixtureArguments = publicBenchmark ? ["--public-benchmark"] : []
  try start(
    fixture, executable: "/usr/bin/python3",
    arguments: [script.path, directory.path] + fixtureArguments)
  let port = try await readFile(directory.appending(path: "port"))
  let executable = try #require(
    ProcessInfo.processInfo.environment["CTXFAB_TEST_BACKEND_EXECUTABLE"])
  let data = directory.appending(path: "data")
  if answerWorker {
    let inference = try stageAnswerWorker(in: data)
    var environment = ProcessInfo.processInfo.environment
    environment["CTXFAB_TEST_PREVIEW_RUNTIME"] = "1"
    environment["CTXFAB_INFERENCE_DIRECTORY"] = inference.path
    backend.environment = environment
  }
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
  try await run(session, directory)
}

private func stageAnswerWorker(in data: URL) throws -> URL {
  let inference = data.appending(path: "inference")
  try FileManager.default.createDirectory(
    at: data, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
  try FileManager.default.createDirectory(at: inference, withIntermediateDirectories: false)
  let script = URL(filePath: #filePath).deletingLastPathComponent()
    .appending(path: "../../../../../quality/answers/mock_worker.py").standardizedFileURL
  let worker = inference.appending(path: "llama-server")
  try FileManager.default.copyItem(at: script, to: worker)
  try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: worker.path)
  try Data().write(to: inference.appending(path: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"))
  return inference
}

private func verifyCommands(_ session: SessionClient, origin: URL) async throws {
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
    intervalMinutes: connected.defaults.intervalMinutes,
    features: .init(currentCode: true, history: true, actions: true, codeGraph: false))
  guard case .saved(let saved) = try await GitHubCommand.save(selection).perform(using: session)
  else {
    Issue.record("Expected saved repository.")
    return
  }
  #expect(saved.repositories.first?.contents == .available)
  #expect(try await session.status().connectedSources == 1)
  let queued = try await verifyCodeQueue(saved, repositoryID: repository.id, using: session)
  let verified = try await verifySavedAccess(queued, using: session)
  try await verifyBranchPages(session, repositoryID: repository.id, revision: verified.revision)
  try await verifyEvidenceFamilies(session, origin: origin, repositoryId: repository.id)
  let finalRevision = try await verifyStorageControls(session, repositoryId: repository.id)
  #expect(finalRevision >= verified.revision)
  let disconnected = try await removeRepositoryAndDisconnect(
    session, repositoryId: repository.id, revision: finalRevision)
  try await verifyRejections(session, selection: selection)
  try await verifyRateLimit(disconnected, using: session)
}

private func verifyBranchPages(_ session: SessionClient, repositoryID: Int64, revision: Int)
  async throws
{
  let first = try await GitHubCommand.branches(id: repositoryID, revision: revision, page: 1)
    .perform(using: session)
  #expect(first == .branches(.init(branches: ["main", "feature/search"], nextPage: 2)))
  let last = try await session.githubBranches(id: repositoryID, revision: revision, page: 2)
  #expect(last == .init(branches: ["release/v1"]))
}

private func removeRepositoryAndDisconnect(
  _ session: SessionClient, repositoryId: Int64, revision: Int
) async throws -> Components.Schemas.GitHubState {
  let removal = GitHubCommand.remove(id: repositoryId, revision: revision)
  guard case .removed(let removed) = try await removal.perform(using: session) else {
    throw URLError(.badServerResponse)
  }
  #expect(removed.repositories.isEmpty)
  let disconnect = GitHubCommand.disconnect(revision: removed.revision)
  guard case .credential(let disconnected) = try await disconnect.perform(using: session) else {
    throw URLError(.badServerResponse)
  }
  #expect(disconnected.credential == .unconfigured)
  return disconnected
}

private func verifyEvidenceFamilies(_ session: SessionClient, origin: URL, repositoryId: Int64)
  async throws
{
  for flag in ["collect-history", "history-modified", "advance-code"] {
    try Data().write(to: origin.appending(path: flag))
  }
  let history = try await session.refreshGitHubHistory(id: repositoryId)
  let child = try #require(history.commits.first)
  #expect(child.id == String(repeating: "b", count: 40))
  #expect(child.parents.first == String(repeating: "a", count: 40))
  try await session.materializeGitHubHistory(id: repositoryId, child: child.id)
  await #expect(throws: GitHubRejection.self) {
    try await session.materializeGitHubHistory(
      id: repositoryId, child: String(repeating: "f", count: 40))
  }
  try FileManager.default.removeItem(at: origin.appending(path: "advance-code"))
  for flag in ["actions-fixture", "actions-completed"] {
    try Data().write(to: origin.appending(path: flag))
  }
  let initial = try await session.gitHubActions(id: repositoryId)
  #expect(initial.availability == .ready && initial.snapshot == nil)
  let collected = try await session.refreshGitHubActions(id: repositoryId)
  let firstRun = try #require(collected.snapshot?.runs.first)
  #expect(firstRun.id == 30 && firstRun.attempt == 1 && firstRun.conclusion == "success")
  await #expect(throws: GitHubRejection.self) {
    _ = try await session.gitHubActions(id: -1)
  }
  await #expect(throws: GitHubRejection.self) {
    _ = try await session.refreshGitHubActions(id: -1)
  }
  let graph = try await session.githubGraph(
    id: repositoryId, branch: "main", root: nil, symbol: nil)
  #expect(graph.snapshot == nil)
}

private func verifyStorageControls(_ session: SessionClient, repositoryId: Int64) async throws
  -> Int
{
  let budget = try await session.githubStorage()
  #expect((1...1000).contains(budget.limitGb))
  let updated = try await session.updateGitHubStorage(limitGb: 12, revision: budget.revision)
  #expect(updated.limitGb == 12)
  #expect(try await session.githubDataPaused(id: repositoryId) == false)
  _ = try await session.purgeGitHubData(id: repositoryId)
  #expect(try await session.githubDataPaused(id: repositoryId))
  #expect(
    try await session.githubState().repositories.contains { $0.repository.id == repositoryId })
  _ = try await GitHubCommand.refreshCode(repositoryId).perform(using: session)
  #expect(try await session.githubDataPaused(id: repositoryId) == false)

  let state = try await session.githubState()
  let configured = try #require(state.repositories.first { $0.repository.id == repositoryId })
  let selection = Components.Schemas.GitHubSelection(
    revision: state.revision, repository: configured.repository.fullName, expectedId: repositoryId,
    branch: configured.branch, intervalMinutes: configured.intervalMinutes,
    features: .init(currentCode: false, history: true, actions: false, codeGraph: false))
  guard
    case .saved(let historyOnly) = try await GitHubCommand.save(selection).perform(using: session)
  else { throw URLError(.badServerResponse) }
  _ = try await session.purgeGitHubData(id: repositoryId)
  #expect(try await session.githubDataPaused(id: repositoryId))
  guard
    case .historyRefreshed(let refreshedId) =
      try await GitHubCommand.refreshHistory(repositoryId).perform(using: session),
    refreshedId == repositoryId
  else { throw URLError(.badServerResponse) }
  #expect(try await session.githubDataPaused(id: repositoryId) == false)
  let final = try await session.githubState()
  #expect(final.repositories.first?.features == selection.features)
  #expect(final.revision == historyOnly.revision)
  return final.revision
}

private func verifyCodeQueue(
  _ saved: Components.Schemas.GitHubState, repositoryID: Int64, using session: SessionClient
) async throws -> Components.Schemas.GitHubState {
  guard
    case .codeRefreshed(_, let queued) = try await GitHubCommand.refreshCode(repositoryID)
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
  let verification = GitHubCommand.verify(id: configured.repository.id, revision: saved.revision)
  let result = try await verification.perform(using: session)
  let snapshot: Components.Schemas.GitHubState? =
    if case .verified(let snapshot) = result { snapshot } else { nil }
  let verified = try #require(snapshot)
  #expect(verified.repositories == saved.repositories)
  #expect(verified.revision == saved.revision)
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
  let defaults = Components.Schemas.GitHubDefaults(
    intervalMinutes: 60, features: features,
    refreshInterval: .init(minimumMinutes: 15, maximumMinutes: 1440))
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
    $0.draftBaseline = .init(revision: snapshot.revision, configured: nil)
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
  guard var edited = store.state.draft else {
    Issue.record("Expected a repository draft")
    return
  }
  edited.intervalInput = "10"
  await store.send(.change(edited)) { $0.draft = edited }
  #expect(!store.state.canSaveRefreshInterval)
  await store.send(.save)

  edited.intervalInput = "15"
  edited.features.actions = true
  await store.send(.change(edited)) { $0.draft = edited }
  #expect(store.state.canSaveRefreshInterval)
  await store.send(.save) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.draft = nil
    $0.draftBaseline = nil
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
        #expect(selection.intervalMinutes == 15)
        #expect(selection.features.actions)
        return .saved(snapshot)
      default:
        return .snapshot(snapshot)
      }
    }
  }
}

private struct RepositoryRevisionFixture {
  let snapshot: Components.Schemas.GitHubState
  let latest: Components.Schemas.GitHubState
  let saved: Components.Schemas.GitHubState
  let message = "GitHub settings changed. Refresh and try again."

  init() {
    let features = Components.Schemas.GitHubFeatures(
      currentCode: true, history: false, actions: false, codeGraph: false)
    let configured = Components.Schemas.GitHubConfiguredRepository(
      repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: true),
      branch: "main", intervalMinutes: 60, features: features,
      contents: .available, actions: .unchecked, verified: true
    )
    snapshot = .init(
      revision: 2, credential: .connected, authorizationUrl: "https://github.com",
      actionsAuthorizationUrl: "https://github.com",
      defaults: .init(intervalMinutes: 60, features: features),
      repositories: [configured]
    )
    var concurrent = snapshot
    concurrent.revision = 3
    concurrent.repositories[0].branch = "release"
    concurrent.repositories[0].features.history = true
    latest = concurrent
    concurrent.revision = 4
    concurrent.repositories[0].intervalMinutes = 45
    concurrent.repositories[0].features.actions = true
    saved = concurrent
  }
}

@MainActor
private func saveConflictStore(_ fixture: RepositoryRevisionFixture) -> TestStoreOf<GitHubConnector>
{
  var initial = GitHubConnector.State()
  initial.managing = true
  initial.snapshot = fixture.snapshot
  return TestStore(initialState: initial) {
    GitHubConnector { command in
      switch command {
      case .save(let selection):
        #expect(selection.intervalMinutes == 45)
        #expect(selection.expectedId == 41)
        #expect(selection.features.actions)
        if selection.revision == fixture.snapshot.revision {
          #expect(selection.branch == "main")
          #expect(!selection.features.history)
          throw GitHubRejection(code: "github_revision_conflict", message: fixture.message)
        }
        #expect(selection.revision == fixture.latest.revision)
        #expect(selection.branch == "release")
        #expect(selection.features.history)
        return .saved(fixture.saved)
      case .load: return .snapshot(fixture.latest)
      default: throw URLError(.badServerResponse)
      }
    }
  }
}

@MainActor
private func beginConflictingDraft(
  _ store: TestStoreOf<GitHubConnector>, fixture: RepositoryRevisionFixture
) async {
  let repository = fixture.snapshot.repositories[0]
  await openRepository(repository, revision: fixture.snapshot.revision, in: store)
  var edited = GitHubConnector.Draft(repository)
  edited.intervalInput = "45"
  edited.features.actions = true
  await store.send(.change(edited)) { $0.draft = edited }
  await store.send(.save) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = fixture.latest
    $0.feedback = fixture.message
  }
  #expect(store.state.draft == edited)
  #expect(store.state.requiresDraftReview)
}

@MainActor
@Test
func repositorySaveConflictUsesLatestAfterReviewingEdits() async {
  let fixture = RepositoryRevisionFixture()
  let store = saveConflictStore(fixture)
  await beginConflictingDraft(store, fixture: fixture)
  await store.send(.collectionPolled(fixture.snapshot))
  await store.send(.save)
  let latestDraft = GitHubConnector.Draft(fixture.latest.repositories[0])
  await store.send(.resolveDraft(.keepEdits)) {
    $0.draft = .init(fixture.saved.repositories[0])
    $0.draftBaseline = .init(revision: fixture.latest.revision, configured: latestDraft)
    $0.feedback = nil
  }
  #expect(!store.state.requiresDraftReview)
  await store.send(.save) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = fixture.saved
    $0.draft = nil
    $0.draftBaseline = nil
    $0.panel = nil
  }
}

@MainActor
@Test
func repositorySaveConflictCanDiscardEditsAndUseLatestSettings() async {
  let fixture = RepositoryRevisionFixture()
  let store = saveConflictStore(fixture)
  await beginConflictingDraft(store, fixture: fixture)
  let latestDraft = GitHubConnector.Draft(fixture.latest.repositories[0])
  await store.send(.resolveDraft(.useLatest)) {
    $0.draft = latestDraft
    $0.draftBaseline = .init(revision: fixture.latest.revision, configured: latestDraft)
    $0.feedback = nil
  }
  #expect(!store.state.requiresDraftReview)
  await store.send(.dismissPanel) {
    $0.draft = nil
    $0.draftBaseline = nil
    $0.panel = nil
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
  await openRepository(repo, revision: active.revision, in: store)
  await store.send(.dismissPanel) {
    $0.draft = nil
    $0.draftBaseline = nil
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
private func openRepository(
  _ repository: Components.Schemas.GitHubConfiguredRepository, revision: Int,
  in store: TestStoreOf<GitHubConnector>
) async {
  let draft = GitHubConnector.Draft(repository)
  await store.send(.edit(repository)) {
    $0.panel = .editOptions
    $0.draft = draft
    $0.draftBaseline = .init(revision: revision, configured: draft)
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
    $0.draftBaseline = .init(revision: 1, configured: nil)
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
    $0.draftBaseline = nil
    $0.panel = nil
  }
  await store.send(.openAccount) { $0.panel = .account }
  await store.send(.authorize(.init("invalid"))) { $0.busy = true }
  await store.receive(\.failed) {
    $0.busy = false
    $0.feedback = "Token rejected."
  }
}

@MainActor
@Test
func storageBudgetAndConfirmedDataDeletionUseExactRepositoryScope() async {
  let features = Components.Schemas.GitHubFeatures(
    currentCode: true, history: true, actions: true, codeGraph: false)
  let repo = Components.Schemas.GitHubConfiguredRepository(
    repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: true),
    branch: "main", intervalMinutes: 60, features: features,
    contents: .available, actions: .available, verified: true)
  let snapshot = Components.Schemas.GitHubState(
    revision: 2, credential: .connected, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com",
    defaults: .init(intervalMinutes: 60, features: features), repositories: [repo])
  let initialBudget = Components.Schemas.GitHubStorageBudget(revision: 1, limitGb: 10, bytes: 1234)
  let updatedBudget = Components.Schemas.GitHubStorageBudget(revision: 2, limitGb: 20, bytes: 1234)
  let purgedBudget = Components.Schemas.GitHubStorageBudget(revision: 2, limitGb: 20, bytes: 0)
  var initial = GitHubConnector.State()
  initial.snapshot = snapshot
  initial.managing = true
  let store = TestStore(initialState: initial) {
    GitHubConnector { command in
      switch command {
      case .loadStorage: return .storage(initialBudget)
      case .setStorage(limitGb: 20, revision: 1): return .storage(updatedBudget)
      case .repositorySettings(41):
        return .repositorySettings(41, paused: false, actions: .init(availability: .ready))
      case .purgeData(41): return .purged(41, purgedBudget)
      default: throw URLError(.badServerResponse)
      }
    }
  }
  await verifyBudgetActions(store, initial: initialBudget, updated: updatedBudget)
  await verifyPurgeActions(store, repo: repo, purged: purgedBudget)

}

@MainActor
private func verifyBudgetActions(
  _ store: TestStoreOf<GitHubConnector>, initial: Components.Schemas.GitHubStorageBudget,
  updated: Components.Schemas.GitHubStorageBudget
) async {
  await store.send(.openTraffic) {
    $0.panel = .traffic
    $0.busy = true
  }
  await store.receive(\.result) {
    $0.busy = false
    $0.storage = initial
  }
  await store.send(.setStorage(0))
  await store.send(.setStorage(20)) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.storage = updated
  }
}

@MainActor
private func verifyPurgeActions(
  _ store: TestStoreOf<GitHubConnector>, repo: Components.Schemas.GitHubConfiguredRepository,
  purged: Components.Schemas.GitHubStorageBudget
) async {
  await store.send(.dismissPanel) { $0.panel = nil }
  await openRepository(repo, revision: 2, in: store)
  await store.send(.loadDataState(41)) {
    $0.dataStateId = 41
    $0.busy = true
  }
  await store.receive(\.result) {
    $0.busy = false
    $0.dataPaused = false
    $0.actionsRepositoryId = 41
    $0.actionsState = .init(availability: .ready)
  }
  await store.send(.purgeData(42))
  await store.send(.purgeData(41)) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.storage = purged
    $0.dataPaused = true
  }
}

@MainActor
@Test
func historyOnlyRepositoryResumesAfterDataDeletion() async {
  let features = Components.Schemas.GitHubFeatures(
    currentCode: false, history: true, actions: false, codeGraph: false)
  let repo = Components.Schemas.GitHubConfiguredRepository(
    repository: .init(id: 41, fullName: "fixture/history", defaultBranch: "main", _private: true),
    branch: "main", intervalMinutes: 60, features: features,
    contents: .available, actions: .unchecked, verified: true)
  let snapshot = Components.Schemas.GitHubState(
    revision: 2, credential: .connected, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com",
    defaults: .init(intervalMinutes: 60, features: features), repositories: [repo])
  var initial = GitHubConnector.State()
  initial.snapshot = snapshot
  initial.panel = .editOptions
  initial.draft = .init(repo)
  initial.dataStateId = 41
  initial.dataPaused = true
  let store = TestStore(initialState: initial) {
    GitHubConnector { command in
      guard case .refreshHistory(41) = command else { throw URLError(.badServerResponse) }
      return .historyRefreshed(41)
    }
  }
  await store.send(.refreshHistory(42))
  await store.send(.refreshHistory(41)) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.dataPaused = false
    $0.feedback = "Code history refreshed."
  }
}
