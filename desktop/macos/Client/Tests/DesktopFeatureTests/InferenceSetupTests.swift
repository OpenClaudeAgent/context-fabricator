import ComposableArchitecture
import LocalAPI
import Testing

@testable import DesktopFeature

@MainActor
@Test
func setupRequiresExplicitConfirmationAndCancelNeverStarts() async {
  let selection = Components.Schemas.InferenceSetupSelection(
    embedding: true, runtimeSource: .hosted)
  let plan = Components.Schemas.InferenceSetupPlan(
    selection: selection, fingerprint: "plan-1", peakAdditionalBytes: 100,
    maximumTransferBytes: 80, availableBytes: 200, canStart: true, runtimeRequired: true,
    answerInstalled: false)
  let starts = LockIsolated(0)
  let store = TestStore(initialState: InferenceSetup.State()) {
    InferenceSetup(
      client: .init(
        plan: { _ in plan },
        start: { _, _, _, _ in
          starts.withValue { $0 += 1 }
          throw BackendConnectionError.helperExited
        }, status: { nil }))
  }
  await store.send(.review(selection, nil)) { $0.requesting = true }
  await store.receive(\.planLoaded) {
    $0.requesting = false
    $0.plan = plan
  }
  #expect(starts.value == 0)
  await store.send(.cancel) { $0.plan = nil }
  await store.send(.confirm)
  #expect(starts.value == 0)
}

@MainActor
@Test
func setupConfirmationSubmitsReviewedFingerprintAndShowsProgress() async {
  let selection = Components.Schemas.InferenceSetupSelection(
    answerProfile: "alternative", embedding: true, embeddingProfile: "f16", runtimeSource: .hosted)
  let plan = Components.Schemas.InferenceSetupPlan(
    selection: selection, fingerprint: "reviewed", peakAdditionalBytes: 50,
    maximumTransferBytes: 30, availableBytes: 100, canStart: true, runtimeRequired: true,
    answerInstalled: false)
  let status = Components.Schemas.InferenceSetupStatus(
    operationId: "00000000-0000-0000-0000-000000000000", phase: .runtime, answerReady: false)
  var initial = InferenceSetup.State()
  initial.plan = plan
  let store = TestStore(initialState: initial) {
    InferenceSetup(
      client: .init(
        plan: { _ in plan },
        start: { submitted, fingerprint, _, path in
          #expect(submitted == selection)
          #expect(fingerprint == "reviewed")
          #expect(path == nil)
          return status
        }, status: { nil }))
  } withDependencies: {
    $0.uuid = .incrementing
  }
  await store.send(.confirm) {
    $0.plan = nil
    $0.requesting = true
    $0.starting = true
  }
  await store.receive(\.statusLoaded) {
    $0.requesting = false
    $0.starting = false
    $0.status = status
  }
  let completed = Components.Schemas.InferenceSetupStatus(
    operationId: status.operationId, phase: .completed, answerReady: true)
  await store.send(.statusPolled(completed)) { $0.status = completed }
}

@MainActor
@Test
func staleSetupPlanRequiresAnotherReview() async {
  let selection = Components.Schemas.InferenceSetupSelection(
    embedding: false, runtimeSource: .hosted)
  let plan = Components.Schemas.InferenceSetupPlan(
    selection: selection, fingerprint: "stale", peakAdditionalBytes: 50,
    maximumTransferBytes: 30, availableBytes: 100, canStart: true, runtimeRequired: true,
    answerInstalled: false)
  var initial = InferenceSetup.State()
  initial.plan = plan
  let starts = LockIsolated(0)
  let store = TestStore(initialState: initial) {
    InferenceSetup(
      client: .init(
        plan: { _ in plan },
        start: { _, _, _, _ in
          starts.withValue { $0 += 1 }
          throw BackendConnectionError.helperExited
        }, status: { nil }))
  } withDependencies: {
    $0.uuid = .incrementing
  }
  await store.send(.confirm) {
    $0.plan = nil
    $0.requesting = true
    $0.starting = true
  }
  await store.receive(\.failed) {
    $0.requesting = false
    $0.starting = false
    $0.feedback = BackendConnectionError.helperExited.localizedDescription
  }
  await store.send(.confirm)
  #expect(starts.value == 1)
}

@MainActor
@Test
func setupPollsProgressUntilCompleted() async {
  let clock = TestClock()
  let selection = Components.Schemas.InferenceSetupSelection(
    embedding: false, runtimeSource: .hosted)
  let plan = Components.Schemas.InferenceSetupPlan(
    selection: selection, fingerprint: "plan", peakAdditionalBytes: 1,
    maximumTransferBytes: 1, availableBytes: 10, canStart: true, runtimeRequired: true,
    answerInstalled: false)
  let working = Components.Schemas.InferenceSetupStatus(
    operationId: "00000000-0000-0000-0000-000000000001", phase: .answerWeights, answerReady: false)
  let completed = Components.Schemas.InferenceSetupStatus(
    operationId: working.operationId, phase: .completed, answerReady: true)
  let statuses = LockIsolated([working, completed])
  let store = TestStore(initialState: InferenceSetup.State()) {
    InferenceSetup(
      client: .init(
        plan: { _ in plan }, start: { _, _, _, _ in working },
        status: {
          statuses.withValue { $0.removeFirst() }
        }))
  } withDependencies: {
    $0.continuousClock = clock
  }
  await store.send(.observe) { $0.observing = true }
  await store.receive(\.statusPolled) { $0.status = working }
  await clock.advance(by: .seconds(1))
  await store.receive(\.statusPolled) { $0.status = completed }
  await store.send(.stop) { $0.observing = false }
}

@MainActor
@Test
func recoveredArchiveUsesCurrentOperationAndExplicitCancellation() async {
  let waiting = Components.Schemas.InferenceSetupStatus(
    operationId: "00000000-0000-0000-0000-000000000001", phase: .waitingForArchive,
    answerReady: false)
  let resumed = Components.Schemas.InferenceSetupStatus(
    operationId: waiting.operationId, phase: .runtime, answerReady: false)
  let calls = LockIsolated<[String]>([])
  var initial = InferenceSetup.State()
  initial.status = waiting
  let store = TestStore(initialState: initial) {
    InferenceSetup(
      client: .init(
        plan: { _ in throw BackendConnectionError.helperExited },
        start: { _, _, _, _ in throw BackendConnectionError.helperExited }, status: { nil },
        provideArchive: { id, path in
          calls.withValue { $0.append(id + ":" + path) }
          return resumed
        },
        cancelArchive: { id in
          calls.withValue { $0.append("cancel:" + id) }
          return waiting
        }))
  }
  await store.send(.provideArchive("stale", "/tmp/runtime.zip"))
  await store.send(.provideArchive(waiting.operationId, "/tmp/runtime.zip")) {
    $0.requesting = true
    $0.starting = true
  }
  await store.receive(\.statusLoaded) {
    $0.requesting = false
    $0.starting = false
    $0.status = resumed
  }
  await store.send(.cancelArchive(waiting.operationId))
  #expect(calls.value == [waiting.operationId + ":/tmp/runtime.zip"])
}

@MainActor
@Test
func cancelWaitingSetupTargetsThePolledOperation() async {
  let waiting = Components.Schemas.InferenceSetupStatus(
    operationId: "00000000-0000-0000-0000-000000000002", phase: .waitingForArchive,
    answerReady: false)
  let cancelled = Components.Schemas.InferenceSetupStatus(
    operationId: waiting.operationId, phase: .interrupted, answerReady: false)
  let calls = LockIsolated<[String]>([])
  var initial = InferenceSetup.State()
  initial.status = waiting
  let store = TestStore(initialState: initial) {
    InferenceSetup(
      client: .init(
        plan: { _ in throw BackendConnectionError.helperExited },
        start: { _, _, _, _ in throw BackendConnectionError.helperExited }, status: { nil },
        cancelArchive: { id in
          calls.withValue { $0.append(id) }
          return cancelled
        }))
  }
  await store.send(.cancelArchive("stale"))
  await store.send(.cancelArchive(waiting.operationId)) {
    $0.requesting = true
    $0.starting = true
  }
  await store.receive(\.statusLoaded) {
    $0.requesting = false
    $0.starting = false
    $0.status = cancelled
  }
  #expect(calls.value == [waiting.operationId])
}

@MainActor
@Test
func changingSelectionCancelsAnInFlightReviewWithoutShowingItsError() async {
  let selection = Components.Schemas.InferenceSetupSelection(
    answerProfile: "alternative", embedding: true, embeddingProfile: "f16", runtimeSource: .hosted)
  let clock = TestClock()
  let store = TestStore(initialState: InferenceSetup.State()) {
    InferenceSetup(
      client: .init(
        plan: { _ in
          do { try await clock.sleep(for: .seconds(30)) } catch {
            throw BackendConnectionError.helperExited
          }
          throw BackendConnectionError.helperExited
        },
        start: { _, _, _, _ in throw BackendConnectionError.helperExited }, status: { nil }))
  }
  await store.send(.review(selection, nil)) { $0.requesting = true }
  await store.send(.cancel) { $0.requesting = false }
  await store.finish()
  #expect(store.state.plan == nil && store.state.feedback == nil)
}
