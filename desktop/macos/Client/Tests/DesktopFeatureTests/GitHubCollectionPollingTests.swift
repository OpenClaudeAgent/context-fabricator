import ComposableArchitecture
import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func githubCollectionObservationStopsAfterPublication() async {
  let clock = TestClock()
  let reads = LockIsolated(0)
  let running = collectionSnapshot(.running)
  let published = collectionSnapshot(.succeeded)
  let store = TestStore(initialState: GitHubConnector.State()) {
    GitHubConnector { command in
      guard case .load = command else { throw URLError(.badServerResponse) }
      return .snapshot(
        reads.withValue { count in
          count += 1
          return count == 1 ? running : published
        })
    }
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.workspaceVisibilityChanged(true)) { $0.workspaceVisible = true }
  await store.send(.manage) {
    $0.managing = true
    $0.busy = true
  }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = running
  }
  await clock.advance(by: .seconds(1))
  #expect(reads.value == 1)
  await clock.advance(by: .seconds(1))
  await store.receive(\.collectionPolled, timeout: .seconds(1)) { $0.snapshot = published }
  await clock.advance(by: .seconds(8))
  await store.finish()
  #expect(reads.value == 2)
}

@MainActor
@Test
func githubCollectionObservationRechecksAnOverdueServerDeadline() async {
  let clock = TestClock()
  let start = clock.now
  let reads = LockIsolated(0)
  let idle = collectionSnapshot(.succeeded, nextAtMs: 31_000)
  let running = collectionSnapshot(.running)
  let published = collectionSnapshot(.succeeded, nextAtMs: 201_000)
  let responses = [idle, idle, running, published]
  var state = GitHubConnector.State()
  state.workspaceVisible = true
  let store = TestStore(initialState: state) {
    GitHubConnector { command in
      guard case .load = command else { throw URLError(.badServerResponse) }
      let index = reads.withValue { count in
        defer { count += 1 }
        return count
      }
      return .snapshot(responses[min(index, responses.count - 1)])
    }
  } withDependencies: {
    $0.continuousClock = clock
    $0.date = DateGenerator {
      Date(timeIntervalSince1970: 1 + Double(start.duration(to: clock.now).components.seconds))
    }
  }
  await store.send(.manage) {
    $0.managing = true
    $0.busy = true
  }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = idle
  }
  await clock.advance(by: .seconds(31))
  #expect(reads.value == 1)
  await clock.advance(by: .seconds(1))
  await store.receive(\.collectionPolled)
  await clock.advance(by: .seconds(9))
  #expect(reads.value == 2)
  await clock.advance(by: .seconds(1))
  await store.receive(\.collectionPolled, timeout: .seconds(1)) { $0.snapshot = running }
  await clock.advance(by: .seconds(2))
  await store.receive(\.collectionPolled, timeout: .seconds(1)) { $0.snapshot = published }
  await clock.advance(by: .seconds(100))
  #expect(reads.value == 4)
  await store.send(.workspaceVisibilityChanged(false)) { $0.workspaceVisible = false }
  await store.finish()
}

@MainActor
@Test
func githubCollectionObservationCancelsWhenLeavingManagement() async {
  let clock = TestClock()
  let reads = LockIsolated(0)
  let queued = collectionSnapshot(.queued)
  var state = GitHubConnector.State()
  state.managing = true
  state.snapshot = queued
  let store = TestStore(initialState: state) {
    GitHubConnector { command in
      guard case .load = command else { throw URLError(.badServerResponse) }
      reads.withValue { $0 += 1 }
      return .snapshot(queued)
    }
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.workspaceVisibilityChanged(true)) {
    $0.workspaceVisible = true
    $0.busy = true
  }
  await store.receive(\.result) { $0.busy = false }
  await clock.advance(by: .seconds(1))
  await store.send(.workspaceVisibilityChanged(false)) { $0.workspaceVisible = false }
  await clock.advance(by: .seconds(10))
  #expect(reads.value == 1)
  await store.send(.workspaceVisibilityChanged(true)) {
    $0.workspaceVisible = true
    $0.busy = true
  }
  await store.receive(\.result) { $0.busy = false }
  await store.send(.close) { $0.managing = false }
  await clock.advance(by: .seconds(10))
  await store.finish()
  #expect(reads.value == 2)
}

@MainActor
@Test
func githubCollectionObservationCanRecoverFromReadFailure() async {
  let clock = TestClock()
  let reads = LockIsolated(0)
  let ready = collectionSnapshot(.succeeded)
  let queued = collectionSnapshot(.queued)
  let running = collectionSnapshot(.running)
  let failure = BackendConnectionError.helperExited
  var state = GitHubConnector.State()
  state.managing = true
  state.workspaceVisible = true
  state.snapshot = ready
  let store = TestStore(initialState: state) {
    GitHubConnector { command in
      if case .refreshCode(41) = command { return .codeRefreshed(41, queued) }
      guard case .load = command else { throw URLError(.badServerResponse) }
      let count = reads.withValue {
        $0 += 1
        return $0
      }
      if count == 1 { throw failure }
      return .snapshot(count == 2 ? running : ready)
    }
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.refreshCode(41)) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.snapshot = queued
  }
  await clock.advance(by: .seconds(2))
  await store.receive(\.collectionPollFailed) {
    $0.collectionReadFailure = failure.localizedDescription
  }
  #expect(store.state.snapshot == queued)
  await clock.advance(by: .seconds(10))
  #expect(reads.value == 1)
  await store.send(.reload) { $0.busy = true }
  await store.receive(\.result) {
    $0.busy = false
    $0.collectionReadFailure = nil
    $0.snapshot = running
  }
  await clock.advance(by: .seconds(2))
  await store.receive(\.collectionPolled) { $0.snapshot = ready }
  await store.finish()
  #expect(reads.value == 3)
}

private func collectionSnapshot(
  _ phase: Components.Schemas.GitHubCodeCollection.phasePayload, nextAtMs: Int64? = nil
) -> Components.Schemas.GitHubState {
  let features = Components.Schemas.GitHubFeatures(
    currentCode: true, history: false, actions: false, codeGraph: false)
  let repository = Components.Schemas.GitHubConfiguredRepository(
    repository: .init(id: 41, fullName: "fixture/alpha", defaultBranch: "main", _private: true),
    branch: "main", intervalMinutes: 60, features: features, contents: .available,
    actions: .unchecked, verified: true,
    codeCollection: .init(phase: phase, canRefresh: phase == .succeeded, nextAtMs: nextAtMs)
  )
  return .init(
    revision: 2, credential: .connected, authorizationUrl: "https://github.com",
    actionsAuthorizationUrl: "https://github.com",
    defaults: .init(intervalMinutes: 60, features: features), repositories: [repository]
  )
}
